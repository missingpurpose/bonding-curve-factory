//! Comprehensive test suite for bonding curve factory
//!
//! Tests cover:
//! - Token creation with various parameters
//! - Buy/sell mechanics and pricing
//! - Graduation scenarios
//! - Security edge cases
//! - Cross-contract integration

pub mod factory_tests;
pub mod bonding_curve_tests;
pub mod amm_integration_tests;
pub mod security_tests;
pub mod performance_tests;

use crate::{BaseToken, TokenLaunchParams, CurveParams};
use alkanes_support::context::Context;
use alkanes_support::id::AlkaneId;

/// Create a test context for unit tests
pub fn create_test_context() -> Context {
    Context {
        myself: AlkaneId::new(1, 1000),
        block_height: 800_000,
        timestamp: 1_700_000_000,
        incoming_alkanes: Vec::new(),
    }
}

/// Create default test token parameters
pub fn create_test_token_params() -> TokenLaunchParams {
    TokenLaunchParams {
        name_part1: string_to_u128("TEST"),
        name_part2: string_to_u128("COIN"),
        symbol: string_to_u128("TEST"),
        image_data: vec![0u8; 100], // Mock image
        base_price: Some(4_000_000),
        growth_rate: Some(150),
        graduation_threshold: Some(6_900_000_000),
        max_supply: Some(1_000_000_000),
        base_token: BaseToken::BUSD,
        lp_distribution_strategy: 0,
    }
}

/// Convert string to u128 for token names
pub fn string_to_u128(s: &str) -> u128 {
    let mut bytes = [0u8; 16];
    let s_bytes = s.as_bytes();
    let len = std::cmp::min(s_bytes.len(), 16);
    bytes[..len].copy_from_slice(&s_bytes[..len]);
    u128::from_le_bytes(bytes)
}
