//! Bonding curve trading and pricing tests

use super::*;
use crate::{bonding_curve::CurveCalculator, CurveParams, constants::*};

#[cfg(test)]
mod curve_pricing_tests {
    use super::*;

    #[test]
    fn test_buy_price_calculation_basic() {
        let params = CurveParams {
            base_price: 1_000_000,        // 0.01 BUSD
            growth_rate: 150,             // 1.5%
            graduation_threshold: 10_000_000_000,
            base_token: BaseToken::BUSD,
            max_supply: 1_000_000_000,
        };

        // Test buying first token
        let price = CurveCalculator::calculate_buy_price(0, 1, &params).unwrap();
        assert!(price >= params.base_price);
        assert!(price < params.base_price * 2);

        // Test buying 100 tokens
        let price_100 = CurveCalculator::calculate_buy_price(0, 100, &params).unwrap();
        assert!(price_100 > price * 100); // Should be more due to curve

        // Test buying at higher supply levels
        let price_high = CurveCalculator::calculate_buy_price(100_000, 1000, &params).unwrap();
        assert!(price_high > price_100);
    }

    #[test]
    fn test_sell_price_calculation() {
        let params = CurveParams::default();
        let current_supply = 10_000;

        // Buy price for comparison
        let buy_price = CurveCalculator::calculate_buy_price(current_supply, 100, &params).unwrap();
        
        // Sell price should be slightly less (1% discount)
        let sell_price = CurveCalculator::calculate_sell_price(current_supply + 100, 100, &params).unwrap();
        
        assert!(sell_price < buy_price);
        assert!(sell_price > buy_price * 98 / 100); // Within 2% discount
    }

    #[test]
    fn test_price_at_supply_exponential() {
        let params = CurveParams {
            base_price: 1_000_000,
            growth_rate: 100, // 1% growth
            graduation_threshold: 10_000_000_000,
            base_token: BaseToken::BUSD,
            max_supply: 1_000_000_000,
        };

        let price_0 = CurveCalculator::price_at_supply(0, &params).unwrap();
        let price_10 = CurveCalculator::price_at_supply(10, &params).unwrap();
        let price_100 = CurveCalculator::price_at_supply(100, &params).unwrap();
        let price_1000 = CurveCalculator::price_at_supply(1000, &params).unwrap();

        assert_eq!(price_0, params.base_price);
        assert!(price_10 > price_0);
        assert!(price_100 > price_10);
        assert!(price_1000 > price_100);

        // Verify exponential growth
        let ratio_10 = price_10 as f64 / price_0 as f64;
        let expected_ratio = 1.01_f64.powi(10);
        assert!((ratio_10 - expected_ratio).abs() < 0.1);
    }

    #[test]
    fn test_max_supply_enforcement() {
        let params = CurveParams {
            base_price: 1_000_000,
            growth_rate: 150,
            graduation_threshold: 10_000_000_000,
            base_token: BaseToken::BUSD,
            max_supply: 1000, // Small max supply for testing
        };

        // Should succeed
        let result = CurveCalculator::calculate_buy_price(500, 400, &params);
        assert!(result.is_ok());

        // Should fail - exceeds max supply
        let result = CurveCalculator::calculate_buy_price(500, 600, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceed maximum supply"));
    }

    #[test]
    fn test_sell_validation() {
        let params = CurveParams::default();

        // Cannot sell more than supply
        let result = CurveCalculator::calculate_sell_price(1000, 2000, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot sell more"));

        // Can sell exact supply
        let result = CurveCalculator::calculate_sell_price(1000, 1000, &params);
        assert!(result.is_ok());
    }

    #[test]
    fn test_graduation_criteria_market_cap() {
        let params = CurveParams {
            base_price: 1_000_000,
            growth_rate: 150,
            graduation_threshold: 100_000_000, // Low threshold for testing
            base_token: BaseToken::BUSD,
            max_supply: 1_000_000_000,
        };

        // Low supply, low reserves - should not graduate
        assert!(!CurveCalculator::check_graduation_criteria(100, 1_000_000, &params));

        // High market cap - should graduate
        let high_supply = 1_000_000;
        let price = CurveCalculator::price_at_supply(high_supply, &params).unwrap();
        let market_cap = high_supply * price;
        
        if market_cap >= params.graduation_threshold {
            assert!(CurveCalculator::check_graduation_criteria(high_supply, 0, &params));
        }
    }

    #[test]
    fn test_graduation_criteria_liquidity() {
        let params = CurveParams {
            base_price: 1_000_000,
            growth_rate: 150,
            graduation_threshold: 10_000_000_000,
            base_token: BaseToken::BUSD,
            max_supply: 1_000_000_000,
        };

        // High liquidity reserves - should graduate
        let high_reserves = params.graduation_threshold / 2;
        assert!(CurveCalculator::check_graduation_criteria(1000, high_reserves, &params));
    }

    #[test]
    fn test_price_continuity() {
        let params = CurveParams::default();
        
        // Buy 1000 tokens in one go
        let bulk_price = CurveCalculator::calculate_buy_price(0, 1000, &params).unwrap();
        
        // Buy 1000 tokens in 10 batches of 100
        let mut incremental_cost = 0u128;
        let mut supply = 0u128;
        
        for _ in 0..10 {
            let batch_cost = CurveCalculator::calculate_buy_price(supply, 100, &params).unwrap();
            incremental_cost += batch_cost;
            supply += 100;
        }
        
        // Prices should be very close (within rounding errors)
        let diff = if bulk_price > incremental_cost {
            bulk_price - incremental_cost
        } else {
            incremental_cost - bulk_price
        };
        
        assert!(diff < bulk_price / 1000); // Less than 0.1% difference
    }

    #[test]
    fn test_slippage_calculation() {
        let params = CurveParams::default();
        let current_supply = 100_000;
        
        // Calculate expected tokens for a given payment
        let payment = 10_000_000; // 0.1 BUSD
        
        // Binary search for token amount
        let mut low = 0u128;
        let mut high = params.max_supply - current_supply;
        let mut best_tokens = 0u128;
        
        while low <= high {
            let mid = (low + high) / 2;
            match CurveCalculator::calculate_buy_price(current_supply, mid, &params) {
                Ok(cost) => {
                    if cost <= payment {
                        best_tokens = mid;
                        low = mid + 1;
                    } else {
                        if mid == 0 { break; }
                        high = mid - 1;
                    }
                },
                Err(_) => {
                    if mid == 0 { break; }
                    high = mid - 1;
                }
            }
        }
        
        assert!(best_tokens > 0);
        
        // Verify the calculation
        let actual_cost = CurveCalculator::calculate_buy_price(current_supply, best_tokens, &params).unwrap();
        assert!(actual_cost <= payment);
        
        // Adding one more token should exceed payment
        if best_tokens < params.max_supply - current_supply {
            let cost_plus_one = CurveCalculator::calculate_buy_price(current_supply, best_tokens + 1, &params).unwrap();
            assert!(cost_plus_one > payment);
        }
    }

    #[test]
    fn test_extreme_values() {
        let params = CurveParams {
            base_price: u128::MAX / 1_000_000, // Very high base price
            growth_rate: 10, // Low growth to avoid overflow
            graduation_threshold: u128::MAX / 100,
            base_token: BaseToken::BUSD,
            max_supply: 1000,
        };

        // Should handle without overflow
        let result = CurveCalculator::calculate_buy_price(0, 10, &params);
        assert!(result.is_ok());
        
        // Test with maximum reasonable values
        let params = CurveParams {
            base_price: 100_000_000_000, // 1000 BUSD
            growth_rate: 1000, // 10% growth
            graduation_threshold: 100_000_000_000_000, // $1M
            base_token: BaseToken::BUSD,
            max_supply: 100_000_000_000, // 100B tokens
        };
        
        let result = CurveCalculator::price_at_supply(1_000_000, &params);
        assert!(result.is_ok());
    }
} 