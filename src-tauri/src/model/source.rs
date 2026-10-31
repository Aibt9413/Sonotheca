use serde::{Deserialize, Serialize};

/// 音源标识。`Direct` 是 M1 的直链源，用来先打通整条流水线。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceId {
    Direct,
    Netease,
    Qq,
    Bilibili,
    Navidrome,
}

impl SourceId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Netease => "netease",
            Self::Qq => "qq",
            Self::Bilibili => "bilibili",
            Self::Navidrome => "navidrome",
        }
    }
}

impl std::str::FromStr for SourceId {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "direct" => Ok(Self::Direct),
            "netease" => Ok(Self::Netease),
            "qq" => Ok(Self::Qq),
            "bilibili" => Ok(Self::Bilibili),
            "navidrome" => Ok(Self::Navidrome),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
