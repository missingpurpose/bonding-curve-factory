// Token identification constants
pub const ALKANE_FACTORY_OWNED_TOKEN_ID: u128 =  0x0fff;
pub const ALKANE_FACTORY_FREE_MINT_ID: u128 = 0x0ffe;

// Security constants
pub const MAX_SLIPPAGE_BPS: u128 = 500;              // 5% maximum slippage
pub const MIN_LIQUIDITY_FOR_GRADUATION: u128 = 3_500_000_000; // $35k USD minimum liquidity
pub const MIN_HOLDERS_FOR_GRADUATION: u32 = 100;     // Minimum unique holders
pub const TIME_LOCK_DURATION: u64 = 86400;           // 24 hour time lock

// Trading limits
pub const MIN_BUY_AMOUNT: u128 = 10_000;            // 0.0001 BTC equivalent minimum
pub const MAX_BUY_PERCENTAGE: u128 = 1000;          // 10% of remaining supply per tx
pub const MAX_SELL_PERCENTAGE: u128 = 2000;         // 20% of circulating supply per tx

// Fee constants  
pub const TRADING_FEE_BPS: u128 = 50;               // 0.5% trading fee
pub const GRADUATION_FEE_BPS: u128 = 200;           // 2% graduation fee
pub const FACTORY_DEPLOYMENT_FEE: u128 = 100_000_000; // 0.001 BTC deployment fee

// Economic constants
pub const DEFAULT_BASE_PRICE: u128 = 4_000_000;     // 0.04 BUSD starting price
pub const DEFAULT_GROWTH_RATE: u128 = 150;          // 1.5% growth per token
pub const DEFAULT_GRADUATION_THRESHOLD: u128 = 6_900_000_000; // $69k market cap
pub const DEFAULT_MAX_SUPPLY: u128 = 1_000_000_000; // 1 billion tokens

// AMM integration constants
pub const AMM_INITIAL_LIQUIDITY_RATIO: u128 = 5000; // 50% of reserves for AMM
pub const LP_BURN_PERCENTAGE: u128 = 8000;          // 80% LP burned by default
