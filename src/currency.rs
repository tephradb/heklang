#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Currency {
    pub code: String,
    pub scale: u8,
}

impl Currency {
    pub fn new(code: impl Into<String>, scale: u8) -> Self {
        Self {
            code: code.into(),
            scale,
        }
    }

    pub fn from_code(code: &str) -> Self {
        let code = code.to_uppercase();
        let scale = match code.as_str() {
            "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF"
            | "UGX" | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
            "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
            _ => 2,
        };
        Self { code, scale }
    }
}
