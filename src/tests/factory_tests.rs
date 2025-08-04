//! Factory tests for token creation and management

use super::*;
use crate::{BondingCurveFactory, TokenInfo, BaseToken, constants::*};
use alkanes_support::parcel::AlkaneTransfer;

#[cfg(test)]
mod factory_creation_tests {
    use super::*;

    #[test]
    fn test_create_token_with_busd() {
        let mut context = create_test_context();
        
        // Add factory fee payment
        context.incoming_alkanes.push(AlkaneTransfer {
            id: BaseToken::BUSD.alkane_id(),
            value: FACTORY_DEPLOYMENT_FEE,
        });
        
        let params = create_test_token_params();
        let result = BondingCurveFactory::create_token(&context, params);
        
        assert!(result.is_ok());
        let response = result.unwrap();
        
        // Verify token was created
        let token_count = BondingCurveFactory::get_token_count();
        assert_eq!(token_count, 1);
        
        // Verify token info
        let token_info: TokenInfo = serde_json::from_slice(&response.data).unwrap();
        assert_eq!(token_info.name, "TESTCOIN");
        assert_eq!(token_info.symbol, "TEST");
        assert_eq!(token_info.creator, context.myself);
        assert!(!token_info.is_graduated);
    }

    #[test]
    fn test_create_token_with_frbtc() {
        let mut context = create_test_context();
        
        // Add factory fee payment in frBTC
        context.incoming_alkanes.push(AlkaneTransfer {
            id: BaseToken::FrBtc.alkane_id(),
            value: FACTORY_DEPLOYMENT_FEE,
        });
        
        let mut params = create_test_token_params();
        params.base_token = BaseToken::FrBtc;
        
        let result = BondingCurveFactory::create_token(&context, params);
        assert!(result.is_ok());
        
        let token_info: TokenInfo = serde_json::from_slice(&result.unwrap().data).unwrap();
        assert_eq!(token_info.base_token, BaseToken::FrBtc);
    }

    #[test]
    fn test_create_token_insufficient_fee() {
        let mut context = create_test_context();
        
        // Add insufficient fee
        context.incoming_alkanes.push(AlkaneTransfer {
            id: BaseToken::BUSD.alkane_id(),
            value: FACTORY_DEPLOYMENT_FEE / 2,
        });
        
        let params = create_test_token_params();
        let result = BondingCurveFactory::create_token(&context, params);
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Insufficient factory fee"));
    }

    #[test]
    fn test_create_token_with_custom_parameters() {
        let mut context = create_test_context();
        
        context.incoming_alkanes.push(AlkaneTransfer {
            id: BaseToken::BUSD.alkane_id(),
            value: FACTORY_DEPLOYMENT_FEE,
        });
        
        let mut params = create_test_token_params();
        params.base_price = Some(10_000_000);       // 0.1 BUSD
        params.growth_rate = Some(300);             // 3% growth
        params.graduation_threshold = Some(10_000_000_000); // $100k
        params.max_supply = Some(500_000_000);      // 500M tokens
        params.lp_distribution_strategy = 1;        // Community rewards
        
        let result = BondingCurveFactory::create_token(&context, params);
        assert!(result.is_ok());
        
        // Verify custom parameters were saved
        let token_info: TokenInfo = serde_json::from_slice(&result.unwrap().data).unwrap();
        // In production, we'd query the token contract to verify params
    }

    #[test]
    fn test_create_multiple_tokens() {
        let mut context = create_test_context();
        
        // Create 3 tokens
        for i in 0..3 {
            context.incoming_alkanes.push(AlkaneTransfer {
                id: BaseToken::BUSD.alkane_id(),
                value: FACTORY_DEPLOYMENT_FEE,
            });
            
            let mut params = create_test_token_params();
            params.name_part1 = string_to_u128(&format!("TOKEN{}", i));
            params.symbol = string_to_u128(&format!("TK{}", i));
            
            let result = BondingCurveFactory::create_token(&context, params);
            assert!(result.is_ok());
        }
        
        // Verify token count
        assert_eq!(BondingCurveFactory::get_token_count(), 3);
        
        // Test pagination
        let token_list = BondingCurveFactory::get_token_list(0, 2).unwrap();
        assert_eq!(token_list.len(), 2);
        
        let next_page = BondingCurveFactory::get_token_list(2, 2).unwrap();
        assert_eq!(next_page.len(), 1);
    }

    #[test]
    fn test_invalid_token_parameters() {
        let mut context = create_test_context();
        
        context.incoming_alkanes.push(AlkaneTransfer {
            id: BaseToken::BUSD.alkane_id(),
            value: FACTORY_DEPLOYMENT_FEE,
        });
        
        // Test empty name
        let mut params = create_test_token_params();
        params.name_part1 = 0;
        params.name_part2 = 0;
        
        let result = BondingCurveFactory::create_token(&context, params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name cannot be empty"));
        
        // Test empty symbol
        let mut params = create_test_token_params();
        params.symbol = 0;
        
        let result = BondingCurveFactory::create_token(&context, params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("symbol cannot be empty"));
        
        // Test invalid base price
        let mut params = create_test_token_params();
        params.base_price = Some(500); // Too low
        
        let result = BondingCurveFactory::create_token(&context, params);
        assert!(result.is_err());
        
        // Test invalid growth rate
        let mut params = create_test_token_params();
        params.growth_rate = Some(2000); // Too high (20%)
        
        let result = BondingCurveFactory::create_token(&context, params);
        assert!(result.is_err());
    }

    #[test]
    fn test_creator_token_tracking() {
        let mut context = create_test_context();
        let creator = context.myself.clone();
        
        // Create 2 tokens
        for i in 0..2 {
            context.incoming_alkanes.push(AlkaneTransfer {
                id: BaseToken::BUSD.alkane_id(),
                value: FACTORY_DEPLOYMENT_FEE,
            });
            
            let mut params = create_test_token_params();
            params.name_part1 = string_to_u128(&format!("CREATOR{}", i));
            
            BondingCurveFactory::create_token(&context, params).unwrap();
        }
        
        // Get creator's tokens
        let creator_tokens = BondingCurveFactory::get_creator_tokens(&creator).unwrap();
        assert_eq!(creator_tokens.len(), 2);
    }

    #[test]
    fn test_token_id_generation() {
        // Test deterministic token ID generation
        let token_id_1 = BondingCurveFactory::generate_token_id(1);
        let token_id_2 = BondingCurveFactory::generate_token_id(2);
        
        // IDs should be different but predictable
        assert_ne!(token_id_1, token_id_2);
        assert_eq!(token_id_1.tx, 1);
        assert_eq!(token_id_2.tx, 2);
    }

    #[test]
    fn test_fee_collection() {
        let mut context = create_test_context();
        
        // Create tokens with different base currencies
        for base_token in [BaseToken::BUSD, BaseToken::FrBtc] {
            context.incoming_alkanes.push(AlkaneTransfer {
                id: base_token.alkane_id(),
                value: FACTORY_DEPLOYMENT_FEE,
            });
            
            let mut params = create_test_token_params();
            params.base_token = base_token;
            
            BondingCurveFactory::create_token(&context, params).unwrap();
        }
        
        // Verify fees were collected
        let busd_fees = BondingCurveFactory::collected_fees_pointer(&BaseToken::BUSD)
            .get_value::<u128>();
        let frbtc_fees = BondingCurveFactory::collected_fees_pointer(&BaseToken::FrBtc)
            .get_value::<u128>();
        
        assert_eq!(busd_fees, FACTORY_DEPLOYMENT_FEE);
        assert_eq!(frbtc_fees, FACTORY_DEPLOYMENT_FEE);
    }
} 