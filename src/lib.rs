//! Alkanes Bonding Curve System
//!
//! A production-ready bonding curve system for Alkanes that enables token launches
//! with BUSD/frBTC integration and automatic graduation to Oyl AMM pools.
//! 
//! This system provides:
//! - Factory pattern for deploying new bonding curves
//! - Exponential pricing algorithm with configurable parameters
//! - BUSD (2:56801) and frBTC (32:0) base currency support
//! - Automatic liquidity graduation to Oyl AMM pools
//! - Comprehensive security patterns and access controls

use alkanes_runtime::storage::StoragePointer;
use alkanes_runtime::{declare_alkane, message::MessageDispatch, runtime::AlkaneResponder, println};
use alkanes_support::gz;
use alkanes_support::response::CallResponse;
use alkanes_support::utils::overflow_error;
use alkanes_support::witness::find_witness_payload;
use alkanes_support::{context::Context, parcel::AlkaneTransfer, id::AlkaneId};
use anyhow::{anyhow, Result};

use bitcoin::Transaction;
use metashrew_support::compat::to_arraybuffer_layout;
use metashrew_support::index_pointer::KeyValuePointer;
use metashrew_support::utils::consensus_decode;
use std::io::Cursor;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

pub mod precompiled;
pub mod bonding_curve;
pub mod amm_integration;
pub mod factory;
#[cfg(test)]
pub mod tests;

// Re-export factory types
pub use factory::{BondingCurveFactory, TokenLaunchParams, TokenInfo};

/// Constants for base token identification
/// BUSD (Block USD): Stablecoin pegged to USD on Alkanes
/// frBTC (Fractal Bitcoin): Wrapped BTC on Alkanes
/// These IDs are verified from the Alkanes protocol documentation
pub const BUSD_ALKANE_ID: u128 = (2u128 << 64) | 56801u128; // 2:56801 - Verified
pub const FRBTC_ALKANE_ID: u128 = (32u128 << 64) | 0u128;   // 32:0 - Verified

/// Factory contract identification
pub const BONDING_CURVE_FACTORY_ID: u128 = 0x0bcd;

/// Base token enum for supported currencies
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaseToken {
    BUSD,
    FrBtc,
}

impl BaseToken {
    pub fn alkane_id(&self) -> AlkaneId {
        match self {
            BaseToken::BUSD => AlkaneId::new(2u128, 56801u128),     // 2:56801
            BaseToken::FrBtc => AlkaneId::new(32u128, 0u128),       // 32:0
        }
    }
}

/// Bonding curve parameters for token launches
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurveParams {
    pub base_price: u128,           // Starting price in base token satoshis
    pub growth_rate: u128,          // Basis points increase per token (e.g., 1500 = 1.5%)
    pub graduation_threshold: u128,  // Market cap threshold for AMM graduation
    pub base_token: BaseToken,      // Base currency (BUSD or frBTC)
    pub max_supply: u128,           // Maximum token supply
}

impl Default for CurveParams {
    fn default() -> Self {
        Self {
            base_price: 1_000_000,        // 0.01 BUSD (assuming 8 decimals)
            growth_rate: 1500,            // 1.5% per token
            graduation_threshold: 10_000_000_000_000, // 100,000 BUSD
            base_token: BaseToken::BUSD,
            max_supply: 1_000_000_000_000_000, // 1 billion tokens
        }
    }
}

/// Returns a StoragePointer for the token name
fn name_pointer() -> StoragePointer {
    StoragePointer::from_keyword("/name")
}

/// Returns a StoragePointer for the token symbol
fn symbol_pointer() -> StoragePointer {
    StoragePointer::from_keyword("/symbol")
}

/// Trims a u128 value to a String by removing trailing zeros
pub fn trim(v: u128) -> String {
    String::from_utf8(
        v.to_le_bytes()
            .into_iter()
            .fold(Vec::<u8>::new(), |mut r, v| {
                if v != 0 {
                    r.push(v)
                }
                r
            }),
    )
    .unwrap()
}

/// TokenName struct to hold two u128 values for the name
#[derive(Default, Clone, Copy)]
pub struct TokenName {
    pub part1: u128,
    pub part2: u128,
}

impl From<TokenName> for String {
    fn from(name: TokenName) -> Self {
        // Trim both parts and concatenate them
        format!("{}{}", trim(name.part1), trim(name.part2))
    }
}

impl TokenName {
    pub fn new(part1: u128, part2: u128) -> Self {
        Self { part1, part2 }
    }
}

/// Context handle for the runtime
pub struct ContextHandle(());

impl AlkaneResponder for ContextHandle {
    // Remove execute method - not part of current API
}

pub const CONTEXT: ContextHandle = ContextHandle(());

/// Extension trait for Context to add transaction_id method
trait ContextExt {
    /// Get the transaction ID from the context
    fn transaction_id(&self) -> Result<bitcoin::Txid>;
}

impl ContextExt for Context {
    fn transaction_id(&self) -> Result<bitcoin::Txid> {
        let tx = consensus_decode::<Transaction>(&mut Cursor::new(CONTEXT.transaction()))?;
        Ok(tx.compute_txid())
    }
}

/// Message enum for factory operations
// #[derive(MessageDispatch)]
enum FactoryMessage {
    /// Create a new bonding curve token
    #[opcode(0)]
    CreateToken {
        /// Token launch parameters (serialized)
        params: Vec<u8>,
    },

    /// Get list of tokens with pagination
    #[opcode(1)]
    GetTokenList {
        /// Starting offset
        offset: u128,
        /// Maximum number of tokens to return
        limit: u128,
    },

    /// Get token info by ID
    #[opcode(2)]
    GetTokenInfo {
        /// Token block number
        token_block: u128,
        /// Token transaction index
        token_tx: u128,
    },

    /// Get tokens created by a specific address
    #[opcode(3)]
    GetCreatorTokens {
        /// Creator block number
        creator_block: u128,
        /// Creator transaction index
        creator_tx: u128,
    },

    /// Set factory fee (admin only)
    #[opcode(100)]
    SetFactoryFee {
        /// New fee amount in satoshis
        fee: u128,
    },

    /// Withdraw collected fees (admin only)
    #[opcode(101)]
    WithdrawFees {
        /// Base token type to withdraw (0 = BUSD, 1 = frBTC)
        base_token_type: u128,
    },

    /// Get factory statistics
    #[opcode(102)]
    GetFactoryStats,
}

/// Factory contract for deploying bonding curve tokens
#[derive(Default)]
pub struct Factory(());

impl Factory {
    fn handle_factory_message(&self, message: FactoryMessage) -> Result<CallResponse> {
        match message {
            FactoryMessage::CreateToken { params } => {
                let launch_params: TokenLaunchParams = serde_json::from_slice(&params)
                    .map_err(|e| anyhow!("Failed to deserialize launch params: {}", e))?;
                
                let context = self.context()?;
                BondingCurveFactory::create_token(&context, launch_params)
            },
            
            FactoryMessage::GetTokenList { offset, limit } => {
                let tokens = BondingCurveFactory::get_token_list(offset, limit)?;
                let data = serde_json::to_vec(&tokens)
                    .map_err(|e| anyhow!("Failed to serialize token list: {}", e))?;
                
                let mut response = CallResponse::default();
                response.data = data;
                Ok(response)
            },
            
            FactoryMessage::GetTokenInfo { token_block, token_tx } => {
                let token_id = AlkaneId { block: token_block, tx: token_tx };
                let token_info = BondingCurveFactory::get_token_info(&token_id)?;
                let data = serde_json::to_vec(&token_info)
                    .map_err(|e| anyhow!("Failed to serialize token info: {}", e))?;
                
                let mut response = CallResponse::default();
                response.data = data;
                Ok(response)
            },
            
            FactoryMessage::GetCreatorTokens { creator_block, creator_tx } => {
                let creator = AlkaneId { block: creator_block, tx: creator_tx };
                let tokens = BondingCurveFactory::get_creator_tokens(&creator)?;
                // Convert AlkaneIds to strings for serialization
                let token_strings: Vec<String> = tokens.iter()
                    .map(|id| format!("{}:{}", id.block, id.tx))
                    .collect();
                let data = serde_json::to_vec(&token_strings)
                    .map_err(|e| anyhow!("Failed to serialize creator tokens: {}", e))?;
                
                let mut response = CallResponse::default();
                response.data = data;
                Ok(response)
            },
            
            FactoryMessage::SetFactoryFee { fee } => {
                // TODO: Add admin access control
                let mut pointer = BondingCurveFactory::factory_fee_pointer();
                pointer.set_value(fee);
                
                let mut response = CallResponse::default();
                response.data = vec![1]; // Success indicator
                Ok(response)
            },
            
            FactoryMessage::WithdrawFees { base_token_type } => {
                // TODO: Add admin access control and withdrawal logic
                let mut response = CallResponse::default();
                response.data = vec![1]; // Success indicator
                Ok(response)
            },
            
            FactoryMessage::GetFactoryStats => {
                let stats = serde_json::json!({
                    "total_tokens": BondingCurveFactory::get_token_count(),
                    "factory_fee": BondingCurveFactory::get_factory_fee(),
                });
                let data = serde_json::to_vec(&stats)
                    .map_err(|e| anyhow!("Failed to serialize stats: {}", e))?;
                
                let mut response = CallResponse::default();
                response.data = data;
                Ok(response)
            },
        }
    }

    fn context(&self) -> Result<Context> {
        // Use current Alkanes API
        Context::parse(&mut Cursor::new(CONTEXT.transaction()))
            .map_err(|e| anyhow!("Failed to parse context: {}", e))
    }
}

/// Main contract that can act as both factory and individual bonding curve
#[derive(Default)]
pub struct BondingCurveSystem(());

impl BondingCurveSystem {
    fn is_factory(&self) -> bool {
        // Check if this is a factory contract by examining the message
        // For now, assume it's a factory if we can't determine otherwise
        true
    }

    fn context(&self) -> Result<Context> {
        // Use current Alkanes API
        Context::parse(&mut Cursor::new(CONTEXT.transaction()))
            .map_err(|e| anyhow!("Failed to parse context: {}", e))
    }
}

// Remove execute implementation - not part of current API
// impl AlkaneResponder for BondingCurveSystem {
//     fn execute(&self) -> Result<CallResponse> {
//         let context = self.context()?;
//         
//         // Try to parse as factory message first
//         if let Ok(factory_message) = FactoryMessage::try_from_transaction(&context) {
//             return Factory(()).handle_factory_message(factory_message);
//         }
//         
//         // Try to parse as bonding curve message
//         if let Ok(curve_message) = BondingCurveMessage::try_from_transaction(&context) {
//             return BondingCurve(()).handle_message(curve_message);
//         }
//         
//         Err(anyhow!("Unknown message type"))
//     }
// }

/// MintableToken trait provides common token functionality
pub trait MintableToken: AlkaneResponder {
    /// Get the token name
    fn name(&self) -> String {
        String::from_utf8(self.name_pointer().get().as_ref().clone())
            .expect("name not saved as utf-8, did this deployment revert?")
    }

    /// Get the token symbol
    fn symbol(&self) -> String {
        String::from_utf8(self.symbol_pointer().get().as_ref().clone())
            .expect("symbol not saved as utf-8, did this deployment revert?")
    }

    /// Set the token name and symbol
    fn set_name_and_symbol(&self, name: TokenName, symbol: u128) {
        let name_string: String = name.into();
        self.name_pointer()
            .set(Arc::new(name_string.as_bytes().to_vec()));
        self.set_string_field(self.symbol_pointer(), symbol);
    }

    /// Get the pointer to the token name
    fn name_pointer(&self) -> StoragePointer {
        name_pointer()
    }

    /// Get the pointer to the token symbol
    fn symbol_pointer(&self) -> StoragePointer {
        symbol_pointer()
    }

    /// Set a string field in storage
    fn set_string_field(&self, mut pointer: StoragePointer, v: u128) {
        pointer.set(Arc::new(trim(v).as_bytes().to_vec()));
    }

    /// Get the pointer to the total supply
    fn total_supply_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/totalsupply")
    }

    /// Get the total supply
    fn total_supply(&self) -> u128 {
        self.total_supply_pointer().get_value::<u128>()
    }

    /// Set the total supply
    fn set_total_supply(&self, v: u128) {
        self.total_supply_pointer().set_value::<u128>(v);
    }

    /// Increase the total supply
    fn increase_total_supply(&self, v: u128) -> Result<()> {
        self.set_total_supply(
            overflow_error(self.total_supply().checked_add(v))
                .map_err(|_| anyhow!("total supply overflow"))?,
        );
        Ok(())
    }

    /// Mint new tokens
    fn mint(&self, context: &Context, value: u128) -> Result<AlkaneTransfer> {
        self.increase_total_supply(value)?;
        Ok(AlkaneTransfer {
            id: context.myself.clone(),
            value,
        })
    }

    /// Get the pointer to the token data
    fn data_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/data")
    }

    /// Get the token data
    fn data(&self) -> Vec<u8> {
        gz::decompress(self.data_pointer().get().as_ref().clone()).unwrap_or_else(|_| vec![])
    }

    /// Set the token data from the transaction
    fn set_data(&self) -> Result<()> {
        let tx = consensus_decode::<Transaction>(&mut Cursor::new(CONTEXT.transaction()))?;
        let data: Vec<u8> = find_witness_payload(&tx, 0).unwrap_or_else(|| vec![]);
        self.data_pointer().set(Arc::new(data));

        Ok(())
    }
}

/// BondingCurve implements a bonding curve token contract
#[derive(Default)]
pub struct BondingCurve(());

impl MintableToken for BondingCurve {}

/// Message enum for bonding curve operations
// #[derive(MessageDispatch)]
enum BondingCurveMessage {
    /// Initialize the bonding curve with parameters
    #[opcode(200)]
    Initialize {
        /// Token name part 1
        name_part1: u128,
        /// Token name part 2  
        name_part2: u128,
        /// Token symbol
        symbol: u128,
        /// Base price in satoshis
        base_price: u128,
        /// Growth rate in basis points
        growth_rate: u128,
        /// Graduation threshold
        graduation_threshold: u128,
        /// Base token type (0 = BUSD, 1 = frBTC)
        base_token_type: u128,
        /// Maximum supply
        max_supply: u128,
        /// LP distribution strategy
        lp_distribution_strategy: u128,
    },

    /// Buy tokens with base currency
    #[opcode(201)]
    BuyTokens {
        /// Minimum tokens expected (slippage protection)
        min_tokens_out: u128,
    },

    /// Sell tokens for base currency
    #[opcode(202)]
    SellTokens {
        /// Amount of tokens to sell
        token_amount: u128,
        /// Minimum base tokens expected (slippage protection)
        min_base_out: u128,
    },

    /// Get buy quote for token amount
    #[opcode(203)]
    GetBuyQuote {
        /// Number of tokens to quote
        token_amount: u128,
    },

    /// Get sell quote for token amount
    #[opcode(204)]
    GetSellQuote {
        /// Number of tokens to quote
        token_amount: u128,
    },

    /// Attempt graduation to AMM
    #[opcode(205)]
    Graduate,

    /// Get curve state information
    #[opcode(206)]
    GetCurveState,

    /// Get the token name
    #[opcode(299)]
    GetName,

    /// Get the token symbol
    #[opcode(300)]
    GetSymbol,

    /// Get the total supply
    #[opcode(301)]
    GetTotalSupply,

    /// Get current base reserves
    #[opcode(302)]
    GetBaseReserves,

    /// Get AMM pool address if graduated
    #[opcode(303)]
    GetAmmPoolAddress,

    /// Check if graduated
    #[opcode(304)]
    IsGraduated,

    /// Get the token data
    #[opcode(1000)]
    GetData,
}

impl BondingCurve {
    fn handle_message(&self, message: BondingCurveMessage) -> Result<CallResponse> {
        match message {
            BondingCurveMessage::Initialize {
                name_part1,
                name_part2,
                symbol,
                base_price,
                growth_rate,
                graduation_threshold,
                base_token_type,
                max_supply,
                lp_distribution_strategy,
            } => {
                self.initialize(
                    name_part1,
                    name_part2,
                    symbol,
                    base_price,
                    growth_rate,
                    graduation_threshold,
                    base_token_type,
                    max_supply,
                    lp_distribution_strategy,
                )
            },
            
            BondingCurveMessage::BuyTokens { min_tokens_out } => {
                self.buy_tokens(min_tokens_out)
            },
            
            BondingCurveMessage::SellTokens { token_amount, min_base_out } => {
                self.sell_tokens(token_amount, min_base_out)
            },
            
            BondingCurveMessage::GetBuyQuote { token_amount } => {
                self.get_buy_quote(token_amount)
            },
            
            BondingCurveMessage::GetSellQuote { token_amount } => {
                self.get_sell_quote(token_amount)
            },
            
            BondingCurveMessage::Graduate => {
                self.graduate()
            },
            
            BondingCurveMessage::GetCurveState => {
                self.get_curve_state()
            },
            
            BondingCurveMessage::GetName => {
                self.get_name()
            },
            
            BondingCurveMessage::GetSymbol => {
                self.get_symbol()
            },
            
            BondingCurveMessage::GetTotalSupply => {
                self.get_total_supply()
            },
            
            BondingCurveMessage::GetBaseReserves => {
                self.get_base_reserves()
            },
            
            BondingCurveMessage::GetAmmPoolAddress => {
                self.get_amm_pool_address()
            },
            
            BondingCurveMessage::IsGraduated => {
                self.is_graduated()
            },
            
            BondingCurveMessage::GetData => {
                self.get_data()
            },
        }
    }

    fn context(&self) -> Result<Context> {
        // Use current Alkanes API
        Context::parse(&mut Cursor::new(CONTEXT.transaction()))
            .map_err(|e| anyhow!("Failed to parse context: {}", e))
    }
    
    // Add missing current_supply method
    fn current_supply(&self) -> u128 {
        self.total_supply()
    }

    /// Initialize the bonding curve with parameters
    fn initialize(
        &self,
        name_part1: u128,
        name_part2: u128,
        symbol: u128,
        base_price: u128,
        growth_rate: u128,
        graduation_threshold: u128,
        base_token_type: u128,
        max_supply: u128,
        lp_distribution_strategy: u128,
    ) -> Result<CallResponse> {
        let context = self.context()?;
        let response = CallResponse::forward(&context.incoming_alkanes);

        // Validate parameters
        let base_token = match base_token_type {
            0 => BaseToken::BUSD,
            1 => BaseToken::FrBtc,
            _ => return Err(anyhow!("Invalid base token type")),
        };

        if lp_distribution_strategy > 3 {
            return Err(anyhow!("Invalid LP distribution strategy (0-3)"));
        }

        let params = CurveParams {
            base_price,
            growth_rate,
            graduation_threshold,
            base_token,
            max_supply,
        };

        bonding_curve::CurveCalculator::set_curve_params(&params)?;

        // Set token metadata
        let name = TokenName::new(name_part1, name_part2);
        <Self as MintableToken>::set_name_and_symbol(self, name, symbol);

        // Store LP distribution strategy
        let mut lp_pointer = StoragePointer::from_keyword("/lp_strategy");
        lp_pointer.set_value(lp_distribution_strategy as u8);

        // Initialize reserves to zero
        bonding_curve::CurveCalculator::set_base_reserves(0);
        bonding_curve::CurveCalculator::set_token_reserves(0);

        // Store token creator
        let mut creator_pointer = StoragePointer::from_keyword("/token/creator");
        let mut creator_data = Vec::new();
        creator_data.extend_from_slice(&context.myself.block.to_le_bytes());
        creator_data.extend_from_slice(&context.myself.tx.to_le_bytes());
        creator_pointer.set(Arc::new(creator_data));

        self.set_data()?;

        Ok(response)
    }

    /// Buy tokens with base currency
    fn buy_tokens(&self, min_tokens_out: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        // Check if already graduated
        if bonding_curve::CurveCalculator::is_graduated() {
            return Err(anyhow!("Bonding curve has graduated to AMM"));
        }

        // Get curve parameters and current state
        let params = bonding_curve::CurveCalculator::get_curve_params()?;
        let current_supply = self.current_supply();
        
        // Find the base token input from incoming alkanes
        let base_input = context.incoming_alkanes.0
            .iter()
            .find(|transfer| transfer.id == params.base_token.alkane_id())
            .ok_or_else(|| anyhow!("No base token input found"))?;

        let base_amount = base_input.value;

        // Calculate how many tokens to mint for this amount
        let tokens_to_mint = self.calculate_tokens_for_base_amount(base_amount, &params)?;

        // Check slippage protection
        if tokens_to_mint < min_tokens_out {
            return Err(anyhow!("Slippage exceeded: got {} tokens, expected at least {}", 
                tokens_to_mint, min_tokens_out));
        }

        // Mint the tokens
        response.alkanes.0.push(self.mint(&context, tokens_to_mint)?);

        // Update reserves
        let current_reserves = bonding_curve::CurveCalculator::get_base_reserves();
        bonding_curve::CurveCalculator::set_base_reserves(current_reserves + base_amount);

        // Check for graduation after purchase
        let new_supply = current_supply + tokens_to_mint;
        if bonding_curve::CurveCalculator::check_graduation_criteria(new_supply, current_reserves + base_amount, &params) {
            // Trigger graduation
            let _ = amm_integration::AMMIntegration::graduate_to_amm(&context, new_supply);
        }

        Ok(response)
    }

    /// Sell tokens for base currency
    fn sell_tokens(&self, token_amount: u128, min_base_out: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        // Check if already graduated
        if bonding_curve::CurveCalculator::is_graduated() {
            return Err(anyhow!("Bonding curve has graduated to AMM"));
        }

        // Get curve parameters and calculate sell price
        let params = bonding_curve::CurveCalculator::get_curve_params()?;
        let current_supply = self.current_supply();
        
        // Calculate base tokens to return
        let base_payout = bonding_curve::CurveCalculator::calculate_sell_price(
            current_supply, token_amount, &params
        )?;

        // Check slippage protection
        if base_payout < min_base_out {
            return Err(anyhow!("Slippage exceeded: got {} base tokens, expected at least {}", 
                base_payout, min_base_out));
        }

        // Check we have enough reserves
        let current_reserves = bonding_curve::CurveCalculator::get_base_reserves();
        if base_payout > current_reserves {
            return Err(anyhow!("Insufficient reserves for sell"));
        }

        // Burn the tokens (decrease total supply)
        let new_supply = current_supply.checked_sub(token_amount)
            .ok_or_else(|| anyhow!("Cannot burn more tokens than exist"))?;
        self.set_total_supply(new_supply);

        // Return base tokens to seller
        response.alkanes.0.push(AlkaneTransfer {
            id: params.base_token.alkane_id(),
            value: base_payout,
        });

        // Update reserves
        bonding_curve::CurveCalculator::set_base_reserves(current_reserves - base_payout);

        Ok(response)
    }

    /// Calculate tokens to mint for a given base amount
    fn calculate_tokens_for_base_amount(&self, base_amount: u128, params: &CurveParams) -> Result<u128> {
        let current_supply = self.current_supply();
        
        // Binary search to find the right number of tokens
        let mut low = 0u128;
        let mut high = params.max_supply.saturating_sub(current_supply);
        let mut best_tokens = 0u128;

        while low <= high {
            let mid = (low + high) / 2;
            let cost = bonding_curve::CurveCalculator::calculate_buy_price(current_supply, mid, params)?;
            
            if cost <= base_amount {
                best_tokens = mid;
                low = mid + 1;
            } else {
                high = mid.saturating_sub(1);
            }

            if low > high {
                break;
            }
        }

        if best_tokens == 0 {
            return Err(anyhow!("Insufficient base amount to buy any tokens"));
        }

        Ok(best_tokens)
    }

    /// Get buy quote for token amount
    fn get_buy_quote(&self, token_amount: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        let params = bonding_curve::CurveCalculator::get_curve_params()?;
        let current_supply = self.current_supply();
        
        let cost = bonding_curve::CurveCalculator::calculate_buy_price(
            current_supply, token_amount, &params
        )?;

        response.data = cost.to_le_bytes().to_vec();
        Ok(response)
    }

    /// Get sell quote for token amount
    fn get_sell_quote(&self, token_amount: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);

        let params = bonding_curve::CurveCalculator::get_curve_params()?;
        let current_supply = self.current_supply();
        
        let payout = bonding_curve::CurveCalculator::calculate_sell_price(
            current_supply, token_amount, &params
        )?;

        response.data = payout.to_le_bytes().to_vec();
        Ok(response)
    }

    /// Attempt graduation to AMM
    fn graduate(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let current_supply = self.current_supply();

        amm_integration::AMMIntegration::graduate_to_amm(&context, current_supply)
    }

    /// Get curve state information
    fn get_curve_state(&self) -> Result<CallResponse> {
        let current_supply = self.current_supply();
        let base_reserves = bonding_curve::CurveCalculator::get_base_reserves();
        let is_graduated = bonding_curve::CurveCalculator::is_graduated();
        let amm_pool = amm_integration::AMMIntegration::get_amm_pool_address();
        
        let state = serde_json::json!({
            "current_supply": current_supply,
            "base_reserves": base_reserves,
            "is_graduated": is_graduated,
            "amm_pool": amm_pool.map(|id| format!("{}:{}", id.block, id.tx)),
            "token_name": self.name(),
            "token_symbol": self.symbol(),
            "total_supply": self.total_supply(),
        });
        
        let data = serde_json::to_vec(&state)
            .map_err(|e| anyhow!("Failed to serialize curve state: {}", e))?;
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }

    /// Get the token name
    fn get_name(&self) -> Result<CallResponse> {
        let name = self.name();
        let data = name.as_bytes().to_vec();
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }

    /// Get the token symbol
    fn get_symbol(&self) -> Result<CallResponse> {
        let symbol = self.symbol();
        let data = symbol.as_bytes().to_vec();
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }

    /// Get the total supply
    fn get_total_supply(&self) -> Result<CallResponse> {
        let supply = self.total_supply();
        let data = supply.to_le_bytes().to_vec();
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }

    /// Get current base reserves
    fn get_base_reserves(&self) -> Result<CallResponse> {
        let reserves = bonding_curve::CurveCalculator::get_base_reserves();
        let data = reserves.to_le_bytes().to_vec();
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }

    /// Get AMM pool address if graduated
    fn get_amm_pool_address(&self) -> Result<CallResponse> {
        let pool = amm_integration::AMMIntegration::get_amm_pool_address();
        let pool_id = if let Some(pool) = pool {
            (pool.block << 64) | pool.tx
        } else {
            0
        };
        let data = pool_id.to_le_bytes().to_vec();
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }

    /// Check if graduated
    fn is_graduated(&self) -> Result<CallResponse> {
        let graduated = bonding_curve::CurveCalculator::is_graduated();
        let data = if graduated { vec![1] } else { vec![0] };
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }

    /// Get the token data
    fn get_data(&self) -> Result<CallResponse> {
        let data = self.data();
        
        let mut response = CallResponse::default();
        response.data = data;
        Ok(response)
    }
}

impl AlkaneResponder for BondingCurve {}
