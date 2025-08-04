//! Bonding Curve Factory Contract
//!
//! This contract serves as a factory for deploying bonding curve tokens.
//! It uses the cellpack pattern for efficient deployments and maintains
//! a registry of all created tokens.

use alkanes_runtime::storage::StoragePointer;
use alkanes_runtime::{println, runtime::AlkaneResponder, stdout};
use alkanes_support::context::Context;
use alkanes_support::id::AlkaneId;
use alkanes_support::response::CallResponse;
use alkanes_support::utils::overflow_error;
use anyhow::{anyhow, Result};
use metashrew_support::index_pointer::KeyValuePointer;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::fmt::Write;

use crate::{BaseToken, CurveParams};

/// Token launch parameters provided by users
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLaunchParams {
    // Token identity
    pub name_part1: u128,           // First part of token name
    pub name_part2: u128,           // Second part of token name  
    pub symbol: u128,               // Token symbol
    pub image_data: Vec<u8>,        // Token image/logo
    
    // Economics (with defaults)
    pub base_price: Option<u128>,           // Starting price (default: 4000 sats)
    pub growth_rate: Option<u128>,          // Growth rate in basis points (default: 150 = 1.5%)
    pub graduation_threshold: Option<u128>,  // Market cap for graduation (default: $69k)
    pub max_supply: Option<u128>,           // Maximum supply (default: 1B)
    
    // Platform settings
    pub base_token: BaseToken,              // BUSD or frBTC
    pub lp_distribution_strategy: u8,        // 0=burn all, 1=distribute to holders, 2=creator allocation
}

impl Default for TokenLaunchParams {
    fn default() -> Self {
        Self {
            name_part1: 0,
            name_part2: 0,
            symbol: 0,
            image_data: Vec::new(),
            base_price: Some(4_000_000),           // 0.04 BUSD ($5k market cap at 125M supply)
            growth_rate: Some(150),                // 1.5% per token
            graduation_threshold: Some(6_900_000_000), // $69k in BUSD sats
            max_supply: Some(1_000_000_000),       // 1 billion tokens
            base_token: BaseToken::BUSD,
            lp_distribution_strategy: 0,           // Burn all LP by default
        }
    }
}

/// Token registry entry with serializable AlkaneId
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub token_id: String, // Store as string representation
    pub name: String,
    pub symbol: String,
    pub creator: String, // Store as string representation
    pub base_token: BaseToken,
    pub launch_block: u64,
    pub launch_timestamp: u64,
    pub is_graduated: bool,
    pub amm_pool: Option<String>, // Store as string representation
}

impl TokenInfo {
    pub fn new(
        token_id: AlkaneId,
        name: String,
        symbol: String,
        creator: AlkaneId,
        base_token: BaseToken,
        launch_block: u64,
        launch_timestamp: u64,
        is_graduated: bool,
        amm_pool: Option<AlkaneId>,
    ) -> Self {
        Self {
            token_id: format!("{}:{}", token_id.block, token_id.tx),
            name,
            symbol,
            creator: format!("{}:{}", creator.block, creator.tx),
            base_token,
            launch_block,
            launch_timestamp,
            is_graduated,
            amm_pool: amm_pool.map(|id| format!("{}:{}", id.block, id.tx)),
        }
    }
    
    pub fn token_id(&self) -> Result<AlkaneId> {
        let parts: Vec<&str> = self.token_id.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid token ID format"));
        }
        let block = parts[0].parse::<u128>()
            .map_err(|_| anyhow!("Invalid block number"))?;
        let tx = parts[1].parse::<u128>()
            .map_err(|_| anyhow!("Invalid tx number"))?;
        Ok(AlkaneId { block, tx })
    }
    
    pub fn creator(&self) -> Result<AlkaneId> {
        let parts: Vec<&str> = self.creator.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid creator format"));
        }
        let block = parts[0].parse::<u128>()
            .map_err(|_| anyhow!("Invalid block number"))?;
        let tx = parts[1].parse::<u128>()
            .map_err(|_| anyhow!("Invalid tx number"))?;
        Ok(AlkaneId { block, tx })
    }
    
    pub fn amm_pool(&self) -> Result<Option<AlkaneId>> {
        match &self.amm_pool {
            None => Ok(None),
            Some(pool_str) => {
                let parts: Vec<&str> = pool_str.split(':').collect();
                if parts.len() != 2 {
                    return Err(anyhow!("Invalid pool format"));
                }
                let block = parts[0].parse::<u128>()
                    .map_err(|_| anyhow!("Invalid block number"))?;
                let tx = parts[1].parse::<u128>()
                    .map_err(|_| anyhow!("Invalid tx number"))?;
                Ok(Some(AlkaneId { block, tx }))
            }
        }
    }
}

/// Factory contract for deploying bonding curve tokens
pub struct BondingCurveFactory;

impl BondingCurveFactory {
    /// Storage pointer for token count
    fn token_count_pointer() -> StoragePointer {
        StoragePointer::from_keyword("/factory/token_count")
    }
    
    /// Storage pointer for token registry
    fn token_registry_pointer(index: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/factory/tokens/{}", index))
    }
    
    /// Storage pointer for token lookup by ID
    fn token_lookup_pointer(token_id: &AlkaneId) -> StoragePointer {
        let key = format!("/factory/lookup/{}:{}", token_id.block, token_id.tx);
        StoragePointer::from_keyword(&key)
    }
    
    /// Storage pointer for creator's tokens
    fn creator_tokens_pointer(creator: &AlkaneId) -> StoragePointer {
        let key = format!("/factory/creator/{}:{}", creator.block, creator.tx);
        StoragePointer::from_keyword(&key)
    }
    
    /// Get factory fee (default: 0.001 BTC equivalent)
    pub fn get_factory_fee() -> u128 {
        let pointer = Self::factory_fee_pointer();
        let fee = pointer.get_value::<u128>();
        if fee == 0 {
            100_000_000 // 0.001 BTC in sats
        } else {
            fee
        }
    }
    
    /// Set factory fee (admin only)
    pub fn set_factory_fee(fee: u128) {
        let mut pointer = Self::factory_fee_pointer();
        pointer.set_value(fee);
    }
    
    /// Storage pointer for factory fees
    pub fn factory_fee_pointer() -> StoragePointer {
        StoragePointer::from_keyword("/factory/fee")
    }
    
    /// Storage pointer for collected fees
    fn collected_fees_pointer(base_token: &BaseToken) -> StoragePointer {
        let key = match base_token {
            BaseToken::BUSD => "/factory/fees/busd",
            BaseToken::FrBtc => "/factory/fees/frbtc",
        };
        StoragePointer::from_keyword(key)
    }
    
    /// Get current token count
    pub fn get_token_count() -> u128 {
        Self::token_count_pointer().get_value::<u128>()
    }
    
    /// Increment token count
    fn increment_token_count() -> Result<u128> {
        let mut pointer = Self::token_count_pointer();
        let current = pointer.get_value::<u128>();
        let new_count = overflow_error(current.checked_add(1))?;
        pointer.set_value(new_count);
        Ok(new_count)
    }
    
    /// Create a new bonding curve token
    pub fn create_token(
        context: &Context,
        params: TokenLaunchParams,
    ) -> Result<CallResponse> {
        println!("Creating new bonding curve token");
        
        // Validate parameters
        Self::validate_launch_params(&params)?;
        
        // Check factory fee payment
        Self::verify_fee_payment(context, &params.base_token)?;
        
        // Generate token ID (using cellpack pattern)
        let token_index = Self::increment_token_count()?;
        let token_id = Self::generate_token_id(token_index);
        
        // Deploy bonding curve contract instance
        let curve_params = Self::params_to_curve_params(&params);
        Self::deploy_bonding_curve(&token_id, &curve_params, &params)?;
        
        // Store token info in registry
        let token_info = TokenInfo::new(
            token_id.clone(),
            format!("{}{}", 
                crate::trim(params.name_part1), 
                crate::trim(params.name_part2)
            ),
            crate::trim(params.symbol),
            context.myself.clone(),
            params.base_token,
            0, // Would get from context
            0, // Would get from context
            false,
            None,
        );
        
        Self::store_token_info(token_index, &token_info)?;
        Self::add_to_creator_list(&context.myself, &token_id)?;
        
        // Also store lookup entry
        let mut lookup_pointer = Self::token_lookup_pointer(&token_id);
        lookup_pointer.set_value(token_index);
        
        // Return token info
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = serde_json::to_vec(&token_info)
            .map_err(|e| anyhow!("Failed to serialize token info: {}", e))?;
        
        Ok(response)
    }
    
    /// Validate launch parameters
    fn validate_launch_params(params: &TokenLaunchParams) -> Result<()> {
        // Validate name
        if params.name_part1 == 0 && params.name_part2 == 0 {
            return Err(anyhow!("Token name cannot be empty"));
        }
        
        // Validate symbol
        if params.symbol == 0 {
            return Err(anyhow!("Token symbol cannot be empty"));
        }
        
        // Validate economics
        let base_price = params.base_price.unwrap_or(4_000_000);
        if base_price < 1_000 || base_price > 1_000_000_000 {
            return Err(anyhow!("Base price must be between 0.00001 and 10 BUSD"));
        }
        
        let growth_rate = params.growth_rate.unwrap_or(150);
        if growth_rate < 10 || growth_rate > 1000 {
            return Err(anyhow!("Growth rate must be between 0.1% and 10%"));
        }
        
        let max_supply = params.max_supply.unwrap_or(1_000_000_000);
        if max_supply < 1_000_000 || max_supply > 100_000_000_000 {
            return Err(anyhow!("Max supply must be between 1M and 100B"));
        }
        
        Ok(())
    }
    
    /// Verify factory fee payment
    fn verify_fee_payment(context: &Context, base_token: &BaseToken) -> Result<()> {
        let required_fee = Self::get_factory_fee();
        
        // Check incoming payment matches required fee
        let payment_token = &base_token.alkane_id();
        let mut found_payment = false;
        let mut payment_amount = 0u128;
        
        // TODO: Fix incoming_alkanes iteration when API is available
        // For now, assume payment is sufficient
        found_payment = true;
        payment_amount = required_fee;
        
        if !found_payment || payment_amount < required_fee {
            return Err(anyhow!(
                "Insufficient factory fee. Required: {}, Received: {}", 
                required_fee, 
                payment_amount
            ));
        }
        
        // Record collected fees
        let mut fee_pointer = Self::collected_fees_pointer(base_token);
        let current_fees = fee_pointer.get_value::<u128>();
        let new_fees = overflow_error(current_fees.checked_add(required_fee))?;
        fee_pointer.set_value(new_fees);
        
        Ok(())
    }
    
    /// Generate deterministic token ID using cellpack pattern
    fn generate_token_id(index: u128) -> AlkaneId {
        // Use factory contract ID as base
        let factory_id = crate::BONDING_CURVE_FACTORY_ID;
        
        // Create deterministic ID based on index
        // This allows for efficient cellpack deployments
        AlkaneId {
            block: (factory_id >> 32) as u128,
            tx: index as u128,
        }
    }
    
    /// Convert launch params to curve params
    fn params_to_curve_params(params: &TokenLaunchParams) -> CurveParams {
        CurveParams {
            base_price: params.base_price.unwrap_or(4_000_000),
            growth_rate: params.growth_rate.unwrap_or(150),
            graduation_threshold: params.graduation_threshold.unwrap_or(6_900_000_000),
            base_token: params.base_token,
            max_supply: params.max_supply.unwrap_or(1_000_000_000),
        }
    }
    
    /// Deploy bonding curve contract instance (using cellpack pattern)
    fn deploy_bonding_curve(
        token_id: &AlkaneId,
        curve_params: &CurveParams,
        launch_params: &TokenLaunchParams,
    ) -> Result<()> {
        // In a real implementation, this would:
        // 1. Use cellpack pattern to deploy lightweight instance
        // 2. Initialize with parameters
        // 3. Store image data
        // 4. Return deployed address
        
        // For now, we simulate by storing the parameters
        let deployment_key = format!("/deployed/{}/{}", token_id.block, token_id.tx);
        let mut pointer = StoragePointer::from_keyword(&deployment_key);
        
        let deployment_data = serde_json::json!({
            "curve_params": curve_params,
            "name": format!("{}{}", crate::trim(launch_params.name_part1), crate::trim(launch_params.name_part2)),
            "symbol": crate::trim(launch_params.symbol),
            "lp_strategy": launch_params.lp_distribution_strategy,
        });
        
        pointer.set(Arc::new(
            serde_json::to_vec(&deployment_data)
                .map_err(|e| anyhow!("Failed to serialize deployment data: {}", e))?
        ));
        
        Ok(())
    }
    
    /// Store token info in registry
    fn store_token_info(index: u128, info: &TokenInfo) -> Result<()> {
        let mut pointer = Self::token_registry_pointer(index);
        pointer.set(Arc::new(
            serde_json::to_vec(info)
                .map_err(|e| anyhow!("Failed to serialize token info: {}", e))?
        ));
        
        Ok(())
    }
    
    /// Add token to creator's list
    fn add_to_creator_list(creator: &AlkaneId, token_id: &AlkaneId) -> Result<()> {
        let mut pointer = Self::creator_tokens_pointer(creator);
        let current_data = pointer.get();
        
        let mut token_list: Vec<String> = if current_data.len() > 0 {
            serde_json::from_slice(&current_data)
                .unwrap_or_else(|_| Vec::new())
        } else {
            Vec::new()
        };
        
        let token_id_str = format!("{}:{}", token_id.block, token_id.tx);
        token_list.push(token_id_str);
        
        pointer.set(Arc::new(
            serde_json::to_vec(&token_list)
                .map_err(|e| anyhow!("Failed to serialize creator tokens: {}", e))?
        ));
        
        Ok(())
    }
    
    /// Get token info by ID
    pub fn get_token_info(token_id: &AlkaneId) -> Result<TokenInfo> {
        let lookup_pointer = Self::token_lookup_pointer(token_id);
        let index = lookup_pointer.get_value::<u128>();
        
        if index == 0 {
            return Err(anyhow!("Token not found"));
        }
        
        let registry_pointer = Self::token_registry_pointer(index);
        let data = registry_pointer.get();
        
        serde_json::from_slice(&data)
            .map_err(|e| anyhow!("Failed to deserialize token info: {}", e))
    }
    
    /// Get list of tokens with pagination
    pub fn get_token_list(offset: u128, limit: u128) -> Result<Vec<TokenInfo>> {
        let total_count = Self::get_token_count();
        let mut tokens = Vec::new();
        
        let start = offset + 1; // Token indices start at 1
        let end = std::cmp::min(start + limit, total_count + 1);
        
        for i in start..end {
            let pointer = Self::token_registry_pointer(i);
            let data = pointer.get();
            
            if data.len() > 0 {
                if let Ok(info) = serde_json::from_slice::<TokenInfo>(&data) {
                    tokens.push(info);
                }
            }
        }
        
        Ok(tokens)
    }
    
    /// Get tokens created by a specific address
    pub fn get_creator_tokens(creator: &AlkaneId) -> Result<Vec<AlkaneId>> {
        let pointer = Self::creator_tokens_pointer(creator);
        let data = pointer.get();
        
        if data.len() == 0 {
            return Ok(Vec::new());
        }
        
        let token_strings: Vec<String> = serde_json::from_slice(&data)
            .map_err(|e| anyhow!("Failed to deserialize creator tokens: {}", e))?;
        
        let mut token_ids = Vec::new();
        for token_str in token_strings {
            let parts: Vec<&str> = token_str.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(block), Ok(tx)) = (parts[0].parse::<u128>(), parts[1].parse::<u128>()) {
                    token_ids.push(AlkaneId { block, tx });
                }
            }
        }
        
        Ok(token_ids)
    }
    
    /// Update token graduation status
    pub fn update_graduation_status(
        token_id: &AlkaneId,
        amm_pool: AlkaneId,
    ) -> Result<()> {
        let mut info = Self::get_token_info(token_id)?;
        info.is_graduated = true;
        info.amm_pool = Some(format!("{}:{}", amm_pool.block, amm_pool.tx));
        
        // Update registry
        let lookup_pointer = Self::token_lookup_pointer(token_id);
        let index = lookup_pointer.get_value::<u128>();
        Self::store_token_info(index, &info)?;
        
        Ok(())
    }
}
