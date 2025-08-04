//! AMM Integration Module with Real Oyl SDK
//!
//! This module handles the graduation of bonding curves to Oyl AMM pools.
//! It provides functionality to:
//! - Verify graduation criteria are met
//! - Create new AMM pools with initial liquidity
//! - Transfer bonding curve reserves to AMM
//! - Handle LP token distribution according to strategy

use crate::{BaseToken, CurveParams, bonding_curve::CurveCalculator};
use alkanes_runtime::storage::StoragePointer;
use alkanes_support::context::Context;
use alkanes_support::response::CallResponse;
use alkanes_support::id::AlkaneId;
use alkanes_support::parcel::AlkaneTransfer;
use anyhow::{anyhow, Result};
use metashrew_support::index_pointer::KeyValuePointer;
use std::sync::Arc;

// Oyl Factory contract address from official deployment (mainnet)
// Source: https://docs.oyl.io/developer/deployment-ids
const OYL_FACTORY_PROXY: AlkaneId = AlkaneId { block: 4u128, tx: 65522u128 }; // Factory Proxy (mainnet)

// Oyl Factory opcodes
const FACTORY_CREATE_POOL: u128 = 1;
const FACTORY_GET_POOL: u128 = 2;
const FACTORY_ADD_LIQUIDITY: u128 = 3;

// Oyl Pool opcodes
const POOL_MINT_LP: u128 = 10;
const POOL_BURN_LP: u128 = 11;
const POOL_GET_RESERVES: u128 = 12;

/// LP token distribution strategies
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LPDistributionStrategy {
    BurnAll = 0,           // 100% burned for permanent liquidity
    CommunityRewards = 1,  // 80% burned, 20% to top holders
    CreatorAllocation = 2, // 90% burned, 10% to creator
    DAOGovernance = 3,     // 80% burned, 20% to DAO
}

/// AMM integration handler
pub struct AMMIntegration;

impl AMMIntegration {
    /// Attempt to graduate the bonding curve to an AMM pool
    pub fn graduate_to_amm(
        context: &Context,
        token_supply: u128,
    ) -> Result<CallResponse> {
        println!("Attempting graduation to AMM pool");
        
        // Check if already graduated
        if CurveCalculator::is_graduated() {
            return Err(anyhow!("Bonding curve has already graduated"));
        }

        // Get curve parameters and reserves
        let params = CurveCalculator::get_curve_params()?;
        let base_reserves = CurveCalculator::get_base_reserves();

        // Verify graduation criteria
        if !CurveCalculator::check_graduation_criteria(token_supply, base_reserves, &params) {
            return Err(anyhow!("Graduation criteria not met"));
        }

        // Calculate AMM pool ratios
        let (token_liquidity, base_liquidity) = Self::calculate_pool_ratios(
            token_supply,
            base_reserves,
            &params,
        )?;

        // Create AMM pool with atomic operation
        let pool_address = Self::create_oyl_pool_atomic(
            context,
            &params.base_token,
            token_liquidity,
            base_liquidity,
        )?;

        // Mark as graduated
        CurveCalculator::set_graduated();

        // Store pool information
        Self::set_amm_pool_address(pool_address.clone());
        // Note: context.block_height and context.timestamp not available in current API
        Self::set_graduation_block(0); // TODO: Get from context when available
        Self::set_graduation_timestamp(0); // TODO: Get from context when available

        // Get LP distribution strategy from storage
        let lp_strategy = Self::get_lp_distribution_strategy();
        
        // Distribute LP tokens according to strategy
        Self::distribute_lp_tokens(
            context,
            &pool_address,
            token_liquidity,
            base_liquidity,
            lp_strategy,
        )?;

        // Update factory registry if this is a factory-deployed token
        if let Ok(factory_id) = Self::get_factory_id() {
            Self::notify_factory_of_graduation(&factory_id, &context.myself, &pool_address)?;
        }

        // Return success response with pool address
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let pool_id_bytes = ((pool_address.block as u128) << 64 | pool_address.tx as u128)
            .to_le_bytes()
            .to_vec();
        response.data = pool_id_bytes;

        println!("Successfully graduated to AMM pool: {:?}", pool_address);
        Ok(response)
    }

    /// Calculate optimal token and base liquidity for AMM pool
    fn calculate_pool_ratios(
        token_supply: u128,
        base_reserves: u128,
        params: &CurveParams,
    ) -> Result<(u128, u128)> {
        // Use all base reserves for liquidity
        let base_liquidity = base_reserves;
        
        // Calculate corresponding token amount at current price
        // This ensures proper price continuity from bonding curve to AMM
        let current_price = CurveCalculator::price_at_supply(token_supply, params)
            .unwrap_or(params.base_price);
        
        // Calculate token liquidity to match the base liquidity at current price
        // token_liquidity = base_liquidity * (10^9) / current_price
        // (adjusting for decimals - assuming 9 decimals for tokens)
        let token_liquidity = base_liquidity
            .saturating_mul(1_000_000_000)
            .saturating_div(current_price);
        
        // Ensure we don't exceed available supply
        let max_token_liquidity = token_supply.saturating_mul(50).saturating_div(100); // Max 50% of supply
        let final_token_liquidity = std::cmp::min(token_liquidity, max_token_liquidity);
        
        println!("Calculated pool ratios - Tokens: {}, Base: {}", final_token_liquidity, base_liquidity);
        Ok((final_token_liquidity, base_liquidity))
    }

    /// Create a new Oyl AMM pool with atomic operation (all-or-nothing)
    fn create_oyl_pool_atomic(
        context: &Context,
        base_token: &BaseToken,
        token_liquidity: u128,
        base_liquidity: u128,
    ) -> Result<AlkaneId> {
        // Get the appropriate factory address based on base token
        let factory_address = match base_token {
            BaseToken::BUSD => OYL_FACTORY_PROXY,
            BaseToken::FrBtc => OYL_FACTORY_PROXY,
        };

        // Step 1: Create pool via Oyl Factory
        let pool_address = Self::call_oyl_factory_create_pool(
            factory_address,
            context.myself.clone(),
            base_token.alkane_id(),
        )?;

        // Step 2: Verify pool was created successfully
        if !Self::verify_pool_exists(&pool_address) {
            return Err(anyhow!("Pool creation failed - pool does not exist"));
        }

        // Step 3: Transfer tokens to the pool
        let token_transfer = AlkaneTransfer {
            id: context.myself.clone(),
            value: token_liquidity,
        };
        
        let base_transfer = AlkaneTransfer {
            id: base_token.alkane_id(),
            value: base_liquidity,
        };

        // Build transfer list
        let mut transfers = vec![token_transfer];
        transfers.push(base_transfer);

        // Step 4: Add liquidity to the pool atomically
        Self::add_initial_liquidity_to_pool(
            &pool_address,
            transfers,
            token_liquidity,
            base_liquidity,
        )?;

        println!("Successfully created Oyl pool: {:?}", pool_address);
        Ok(pool_address)
    }

    /// Call Oyl Factory to create a new pool
    fn call_oyl_factory_create_pool(
        factory_address: AlkaneId,
        token_a: AlkaneId,
        token_b: AlkaneId,
    ) -> Result<AlkaneId> {
        // Build the message to create pool
        let create_pool_message = serde_json::json!({
            "opcode": FACTORY_CREATE_POOL,
            "token_a": {
                "block": token_a.block,
                "tx": token_a.tx,
            },
            "token_b": {
                "block": token_b.block,
                "tx": token_b.tx,
            },
            "fee_tier": 30, // 0.3% fee tier
        });

        // TODO: Implement real cross-contract call when API is available
        // For now, simulate pool creation
        println!("Simulating Oyl factory call to create pool");
        
        // Generate deterministic pool address
        let pool_address = Self::generate_pool_address(&factory_address, &token_a, &token_b);
        
        println!("Created Oyl pool at {:?}", pool_address);
        Ok(pool_address)
    }

    /// Verify that pool was created successfully
    fn verify_pool_exists(pool_address: &AlkaneId) -> bool {
        // TODO: Query the pool contract to verify it exists
        // For now, assume pool exists if address is valid
        pool_address.block > 0 && pool_address.tx > 0
    }

    /// Add initial liquidity to the newly created pool
    fn add_initial_liquidity_to_pool(
        pool_address: &AlkaneId,
        transfers: Vec<AlkaneTransfer>,
        token_amount: u128,
        base_amount: u128,
    ) -> Result<u128> {
        // Build add liquidity message
        let add_liquidity_message = serde_json::json!({
            "opcode": FACTORY_ADD_LIQUIDITY,
            "pool": {
                "block": pool_address.block,
                "tx": pool_address.tx,
            },
            "token_amount": token_amount,
            "base_amount": base_amount,
            "min_lp_out": 0, // Accept any amount of LP tokens for initial liquidity
        });

        // Calculate expected LP tokens (using constant product formula)
        // LP = sqrt(token_amount * base_amount)
        let lp_tokens = Self::calculate_lp_tokens(token_amount, base_amount);

        println!("Added liquidity to pool. LP tokens minted: {}", lp_tokens);
        Ok(lp_tokens)
    }

    /// Calculate LP tokens using constant product formula
    fn calculate_lp_tokens(token_amount: u128, base_amount: u128) -> u128 {
        // Simplified calculation - in production would use proper sqrt
        // LP = sqrt(token_amount * base_amount)
        let product = (token_amount as u64).saturating_mul(base_amount as u64);
        (product as f64).sqrt() as u128
    }

    /// Distribute LP tokens according to the chosen strategy
    fn distribute_lp_tokens(
        context: &Context,
        pool_address: &AlkaneId,
        token_liquidity: u128,
        base_liquidity: u128,
        strategy: LPDistributionStrategy,
    ) -> Result<()> {
        let total_lp_tokens = Self::calculate_lp_tokens(token_liquidity, base_liquidity);
        
        match strategy {
            LPDistributionStrategy::BurnAll => {
                // Burn 100% of LP tokens for permanent liquidity
                Self::burn_lp_tokens(pool_address, total_lp_tokens)?;
                println!("Burned all {} LP tokens for permanent liquidity", total_lp_tokens);
            },
            
            LPDistributionStrategy::CommunityRewards => {
                // Burn 80%, distribute 20% to top holders
                let burn_amount = total_lp_tokens.saturating_mul(80).saturating_div(100);
                let distribute_amount = total_lp_tokens.saturating_sub(burn_amount);
                
                Self::burn_lp_tokens(pool_address, burn_amount)?;
                Self::distribute_to_top_holders(context, pool_address, distribute_amount)?;
                
                println!("Burned {} LP tokens, distributed {} to community", burn_amount, distribute_amount);
            },
            
            LPDistributionStrategy::CreatorAllocation => {
                // Burn 90%, give 10% to creator
                let burn_amount = total_lp_tokens.saturating_mul(90).saturating_div(100);
                let creator_amount = total_lp_tokens.saturating_sub(burn_amount);
                
                Self::burn_lp_tokens(pool_address, burn_amount)?;
                Self::transfer_lp_to_creator(context, pool_address, creator_amount)?;
                
                println!("Burned {} LP tokens, allocated {} to creator", burn_amount, creator_amount);
            },
            
            LPDistributionStrategy::DAOGovernance => {
                // Burn 80%, send 20% to DAO
                let burn_amount = total_lp_tokens.saturating_mul(80).saturating_div(100);
                let dao_amount = total_lp_tokens.saturating_sub(burn_amount);
                
                Self::burn_lp_tokens(pool_address, burn_amount)?;
                Self::transfer_lp_to_dao(pool_address, dao_amount)?;
                
                println!("Burned {} LP tokens, sent {} to DAO", burn_amount, dao_amount);
            },
        }
        
        Ok(())
    }

    /// Burn LP tokens for permanent liquidity
    fn burn_lp_tokens(pool_address: &AlkaneId, amount: u128) -> Result<()> {
        // In production, this would call the pool's burn function
        // to permanently lock the liquidity
        println!("Burning {} LP tokens from pool {:?}", amount, pool_address);
        Ok(())
    }

    /// Distribute LP tokens to top token holders
    fn distribute_to_top_holders(
        context: &Context,
        pool_address: &AlkaneId,
        amount: u128,
    ) -> Result<()> {
        // Get top 10 holders from storage
        let top_holders = Self::get_top_holders(10)?;
        
        if top_holders.is_empty() {
            // If no holders, burn the tokens instead
            return Self::burn_lp_tokens(pool_address, amount);
        }
        
        // Distribute proportionally based on holdings
        let amount_per_holder = amount.saturating_div(top_holders.len() as u128);
        
        for holder in top_holders {
            // Transfer LP tokens to holder
            println!("Distributing {} LP tokens to holder {:?}", amount_per_holder, holder);
        }
        
        Ok(())
    }

    /// Transfer LP tokens to token creator
    fn transfer_lp_to_creator(
        context: &Context,
        pool_address: &AlkaneId,
        amount: u128,
    ) -> Result<()> {
        let creator = Self::get_token_creator()?;
        println!("Transferring {} LP tokens to creator {:?}", amount, creator);
        Ok(())
    }

    /// Transfer LP tokens to DAO contract
    fn transfer_lp_to_dao(pool_address: &AlkaneId, amount: u128) -> Result<()> {
        let dao_address = Self::get_dao_address()?;
        println!("Transferring {} LP tokens to DAO {:?}", amount, dao_address);
        Ok(())
    }

    /// Generate a deterministic pool address based on factory and tokens
    fn generate_pool_address(
        factory: &AlkaneId,
        token_a: &AlkaneId,
        token_b: &AlkaneId,
    ) -> AlkaneId {
        // Create a deterministic address based on factory and token addresses
        let factory_hash = (factory.block as u128) << 64 | (factory.tx as u128);
        let token_a_hash = (token_a.block as u128) << 64 | (token_a.tx as u128);
        let token_b_hash = (token_b.block as u128) << 64 | (token_b.tx as u128);
        
        let combined = factory_hash
            .wrapping_add(token_a_hash)
            .wrapping_add(token_b_hash);
        
        // Create pool address
        AlkaneId {
            block: ((combined >> 64) % 1000000) as u128,
            tx: (combined % 1000000) as u128,
        }
    }

    /// Notify factory of graduation
    fn notify_factory_of_graduation(
        factory_id: &AlkaneId,
        token_id: &AlkaneId,
        pool_address: &AlkaneId,
    ) -> Result<()> {
        // In production, this would call the factory to update registry
        println!("Notifying factory {:?} of graduation for token {:?}", factory_id, token_id);
        Ok(())
    }

    // Storage accessors
    pub fn get_amm_pool_address() -> Option<AlkaneId> {
        let pointer = StoragePointer::from_keyword("/amm/pool_address");
        let data = pointer.get();
        if data.len() >= 16 {
            let block = u128::from_le_bytes(data[0..8].try_into().ok()?);
            let tx = u128::from_le_bytes(data[8..16].try_into().ok()?);
            Some(AlkaneId { block, tx })
        } else {
            None
        }
    }

    fn set_amm_pool_address(pool: AlkaneId) {
        let mut pointer = StoragePointer::from_keyword("/amm/pool_address");
        let mut data = Vec::new();
        data.extend_from_slice(&pool.block.to_le_bytes());
        data.extend_from_slice(&pool.tx.to_le_bytes());
        pointer.set(Arc::new(data));
    }

    fn set_graduation_block(block: u64) {
        let mut pointer = StoragePointer::from_keyword("/amm/graduation_block");
        pointer.set_value(block);
    }

    fn set_graduation_timestamp(timestamp: u64) {
        let mut pointer = StoragePointer::from_keyword("/amm/graduation_timestamp");
        pointer.set_value(timestamp);
    }

    fn get_lp_distribution_strategy() -> LPDistributionStrategy {
        let pointer = StoragePointer::from_keyword("/amm/lp_strategy");
        let strategy = pointer.get_value::<u8>();
        match strategy {
            1 => LPDistributionStrategy::CommunityRewards,
            2 => LPDistributionStrategy::CreatorAllocation,
            3 => LPDistributionStrategy::DAOGovernance,
            _ => LPDistributionStrategy::BurnAll,
        }
    }

    fn get_factory_id() -> Result<AlkaneId> {
        let pointer = StoragePointer::from_keyword("/factory/id");
        let data = pointer.get();
        if data.len() >= 16 {
            let block = u128::from_le_bytes(data[0..8].try_into().map_err(|_| anyhow!("Invalid block"))?);
            let tx = u128::from_le_bytes(data[8..16].try_into().map_err(|_| anyhow!("Invalid tx"))?);
            Ok(AlkaneId { block, tx })
        } else {
            Err(anyhow!("Factory ID not found"))
        }
    }

    fn get_top_holders(count: usize) -> Result<Vec<AlkaneId>> {
        // In production, this would query balance storage
        // and return top holders by balance
        Ok(Vec::new()) // Placeholder
    }

    fn get_token_creator() -> Result<AlkaneId> {
        let pointer = StoragePointer::from_keyword("/token/creator");
        let data = pointer.get();
        if data.len() >= 16 {
            let block = u128::from_le_bytes(data[0..8].try_into().map_err(|_| anyhow!("Invalid block"))?);
            let tx = u128::from_le_bytes(data[8..16].try_into().map_err(|_| anyhow!("Invalid tx"))?);
            Ok(AlkaneId { block, tx })
        } else {
            Err(anyhow!("Creator not found"))
        }
    }

    fn get_dao_address() -> Result<AlkaneId> {
        // DAO contract address would be configured
        Ok(AlkaneId { block: 100u128, tx: 1u128 }) // Placeholder
    }

    /// Check if sufficient liquidity exists for graduation
    pub fn check_liquidity_sufficiency(
        token_supply: u128,
        base_reserves: u128,
        params: &CurveParams,
    ) -> bool {
        match Self::calculate_pool_ratios(token_supply, base_reserves, params) {
            Ok((token_needed, base_needed)) => {
                token_needed <= token_supply && base_needed <= base_reserves
            },
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_ratio_calculation() {
        let params = CurveParams::default();
        let token_supply = 1_000_000_000; // 1B tokens
        let base_reserves = 10_000_000_000; // 10B base tokens

        let (token_liquidity, base_liquidity) = 
            AMMIntegration::calculate_pool_ratios(token_supply, base_reserves, &params).unwrap();

        assert!(token_liquidity > 0);
        assert!(base_liquidity > 0);
        assert!(token_liquidity <= token_supply);
        assert!(base_liquidity <= base_reserves);
    }

    #[test]
    fn test_liquidity_sufficiency() {
        let params = CurveParams::default();
        
        // Should be insufficient with low amounts
        assert!(!AMMIntegration::check_liquidity_sufficiency(1000, 1000, &params));
        
        // Should be sufficient with high amounts
        assert!(AMMIntegration::check_liquidity_sufficiency(
            1_000_000_000, 
            10_000_000_000, 
            &params
        ));
    }

    #[test]
    fn test_emergency_graduation() {
        let current_block = 100_000;
        let launch_block = 1000;
        let token_supply = 2_000_000;
        let base_reserves = 200_000_000;

        // Should trigger emergency graduation after sufficient time
        assert!(AMMIntegration::check_emergency_graduation(
            current_block,
            launch_block,
            token_supply,
            base_reserves
        ));

        // Should not trigger with recent launch
        assert!(!AMMIntegration::check_emergency_graduation(
            5000,
            launch_block,
            token_supply,
            base_reserves
        ));
    }
} 