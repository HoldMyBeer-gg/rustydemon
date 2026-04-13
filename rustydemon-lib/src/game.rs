use crate::error::CascError;

/// Known game types that can be stored in a CASC archive.
///
/// Detected from the `build-uid` field in the build config, or from the
/// `Product` field in `.build.info`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GameType {
    HeroesOfTheStorm,
    Hearthstone,
    Warcraft3Reforged,
    StarCraft,
    StarCraft2,
    WorldOfWarcraft,
    DiabloIII,
    DiabloIVBeta,
    DiabloIV,
    Agent,
    Overwatch,
    BattleNet,
    Client,
    Destiny2,
    DiabloIIResurrected,
    Wlby,
    Viper,
    Odin,
    Lazarus,
    Fore,
    Zeus,
    Rtro,
    Anbs,
    DiabloRTL,
    DiabloRTL2,
    Warcraft,
    Warcraft2,
    Gryphon,
}

impl GameType {
    /// Detect the game type from the product UID string (e.g. `"wow"`, `"d3"`).
    pub fn from_uid(uid: &str) -> Result<Self, CascError> {
        if uid.starts_with("hero") {
            return Ok(Self::HeroesOfTheStorm);
        }
        if uid.starts_with("hs") {
            return Ok(Self::Hearthstone);
        }
        if uid.starts_with("w3") {
            return Ok(Self::Warcraft3Reforged);
        }
        if uid.starts_with("s1") || uid.starts_with("sc1") {
            return Ok(Self::StarCraft);
        }
        if uid.starts_with("s2") {
            return Ok(Self::StarCraft2);
        }
        if uid.starts_with("wow") {
            return Ok(Self::WorldOfWarcraft);
        }
        if uid.starts_with("d3") {
            return Ok(Self::DiabloIII);
        }
        if uid.starts_with("agent") {
            return Ok(Self::Agent);
        }
        if uid.starts_with("pro") {
            return Ok(Self::Overwatch);
        }
        if uid.starts_with("bna") {
            return Ok(Self::BattleNet);
        }
        if uid.starts_with("clnt") {
            return Ok(Self::Client);
        }
        if uid.starts_with("dst2") {
            return Ok(Self::Destiny2);
        }
        if uid.starts_with("osi") {
            return Ok(Self::DiabloIIResurrected);
        }
        if uid.starts_with("wlby") {
            return Ok(Self::Wlby);
        }
        if uid.starts_with("viper") {
            return Ok(Self::Viper);
        }
        if uid.starts_with("odin") {
            return Ok(Self::Odin);
        }
        if uid.starts_with("lazr") {
            return Ok(Self::Lazarus);
        }
        if uid.starts_with("fore") {
            return Ok(Self::Fore);
        }
        if uid.starts_with("zeus") {
            return Ok(Self::Zeus);
        }
        if uid.starts_with("rtro") {
            return Ok(Self::Rtro);
        }
        if uid.starts_with("anbs") {
            return Ok(Self::Anbs);
        }
        if uid.starts_with("fenris") {
            return Ok(Self::DiabloIV);
        }
        if uid.starts_with("drtl2") {
            return Ok(Self::DiabloRTL2);
        }
        if uid.starts_with("drtl") {
            return Ok(Self::DiabloRTL);
        }
        if uid.starts_with("war1") {
            return Ok(Self::Warcraft);
        }
        if uid.starts_with("w2bn") {
            return Ok(Self::Warcraft2);
        }
        if uid.starts_with("gryphon") {
            return Ok(Self::Gryphon);
        }
        Err(CascError::UnknownGame(uid.to_owned()))
    }

    /// Returns the game-data sub-folder name relative to the installation root,
    /// e.g. `"Data"` for WoW, `"SC2Data"` for StarCraft II.
    ///
    /// Not every game type has a well-known local layout; those return `None`.
    pub fn data_folder(self) -> Option<&'static str> {
        match self {
            Self::HeroesOfTheStorm => Some("HeroesData"),
            Self::StarCraft => Some("Data"),
            Self::StarCraft2 => Some("SC2Data"),
            Self::Hearthstone => Some("Hearthstone_Data"),
            Self::WorldOfWarcraft
            | Self::DiabloIII
            | Self::DiabloIV
            | Self::Warcraft3Reforged
            | Self::DiabloIIResurrected => Some("Data"),
            Self::Odin => Some("Data"),
            Self::Overwatch => Some("data/casc"),
            _ => None,
        }
    }
}
