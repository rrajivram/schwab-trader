/// Index cap — large indices are truncated to this many symbols (sorted A→Z).
pub const CAP: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Index {
    Sp500,
    Nasdaq100,
    Dow30,
}

impl Index {
    #[allow(dead_code)]
    pub const ALL: &'static [Index] = &[Index::Sp500, Index::Nasdaq100, Index::Dow30];

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Index::Sp500 => "S&P 500",
            Index::Nasdaq100 => "NASDAQ 100",
            Index::Dow30 => "DOW 30",
        }
    }

    pub fn symbols(self) -> Vec<String> {
        let raw: &[&str] = match self {
            Index::Sp500 => SP500,
            Index::Nasdaq100 => NASDAQ100,
            Index::Dow30 => DOW30,
        };
        let mut v: Vec<String> = raw.iter().map(|s| s.to_string()).collect();
        v.sort();
        v.truncate(CAP);
        v
    }
}

// S&P 500 — first 100 alphabetically (A through COF)
const SP500: &[&str] = &[
    "A", "AAL", "AAPL", "ABBV", "ABC", "ABT", "ACN", "ADBE", "ADI", "ADM",
    "ADP", "ADSK", "AEE", "AEP", "AES", "AFL", "AIG", "AIZ", "AJG", "AKAM",
    "ALB", "ALGN", "ALL", "ALLE", "AMAT", "AMCR", "AMD", "AME", "AMGN", "AMP",
    "AMT", "AMZN", "ANET", "ANSS", "AON", "AOS", "APA", "APD", "APH", "APTV",
    "ARE", "ATO", "AVB", "AVGO", "AVY", "AWK", "AXP", "AZO",
    "BA",  "BAC", "BALL", "BAX", "BBY", "BDX", "BEN", "BIIB", "BIO", "BK",
    "BKNG", "BKR", "BLK", "BMY", "BR",  "BRO", "BSX", "BWA",
    "C",   "CAG", "CAH", "CARR", "CAT", "CB",  "CBOE", "CBRE", "CCI", "CCL",
    "CDNS", "CDW", "CEG", "CF",  "CFG", "CHD", "CHRW", "CI",  "CINF", "CL",
    "CLX", "CMA", "CMCSA", "CME", "CMG", "CMI", "CMS", "CNC", "CNP", "COF",
];

// NASDAQ 100 — full 100 constituents (approximate 2025)
const NASDAQ100: &[&str] = &[
    "AAPL", "ABNB", "ADBE", "ADI",  "ADP",  "ADSK", "AMAT", "AMD",  "AMGN", "AMZN",
    "ANSS", "ASML", "AVGO", "AXON", "BIIB", "BKNG", "CDNS", "CDW",  "CEG",  "CHTR",
    "CMCSA","COST", "CPRT", "CRWD", "CSCO", "CSGP", "CSX",  "CTSH", "DDOG", "DLTR",
    "DXCM", "EA",   "EXC",  "FANG", "FAST", "FTNT", "GEHC", "GILD", "GOOG", "GOOGL",
    "HON",  "IDXX", "ILMN", "INTC", "INTU", "ISRG", "KDP",  "KHC",  "KLAC", "LIN",
    "LRCX", "LULU", "MAR",  "MCHP", "MDLZ", "MELI", "META", "MNST", "MRNA", "MRVL",
    "MSFT", "MU",   "NFLX", "NVDA", "NXPI", "ODFL", "ON",   "ORLY", "PANW", "PAYX",
    "PCAR", "PDD",  "PYPL", "QCOM", "REGN", "ROP",  "ROST", "SBUX", "SMCI", "SNPS",
    "TEAM", "TMUS", "TSLA", "TTWO", "TTD",  "TXN",  "VRSK", "VRTX", "WBA",  "WBD",
    "WDAY", "XEL",  "ZS",   "DKNG", "ENPH", "EBAY", "OKTA", "SIRI", "ZM",   "SPLK",
];

// DOW 30 — all 30 constituents
const DOW30: &[&str] = &[
    "AAPL", "AMGN", "AMZN", "AXP", "BA",   "CAT",  "CRM",  "CSCO", "CVX", "DIS",
    "GS",   "HD",   "HON",  "IBM", "JNJ",  "JPM",  "KO",   "MCD",  "MMM", "MRK",
    "MSFT", "NKE",  "NVDA", "PG",  "SHW",  "TRV",  "UNH",  "V",    "VZ",  "WMT",
];
