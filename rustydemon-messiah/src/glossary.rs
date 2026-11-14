//! English for the romanised Chinese (pinyin) words in Diablo Immortal's
//! resource names, so `Mesh_m_crusader_toukui_half_sz06_009` reads as a
//! cosmetic helmet. Only words whose meaning is clear are listed; a name
//! with none of them gets no gloss rather than a guess.

/// Pinyin word (or abbreviation) and its meaning.
const WORDS: &[(&str, &str)] = &[
    // Armour and body.
    ("toukui", "helmet"),
    ("jianjia", "shoulder armour"),
    ("yifu", "body armour"),
    ("tui", "legs"),
    ("bijia", "bracers"),
    ("shoutao", "gloves"),
    ("xiezi", "boots"),
    ("yaodai", "belt"),
    ("pifeng", "cloak"),
    ("qunzi", "skirt"),
    ("mianju", "mask"),
    ("kuijia", "armour"),
    ("toufa", "hair"),
    ("maofa", "hair"),
    ("huzi", "beard"),
    ("tou", "head"),
    ("bozi", "neck"),
    ("shoubi", "arm"),
    ("chibang", "wings"),
    ("yumao", "feathers"),
    ("piaodai", "ribbons"),
    ("benti", "main body"),
    // Weapons and off-hands.
    ("wuqi", "weapon"),
    ("dun", "shield"),
    ("quantao", "fist weapon"),
    ("fazhang", "staff"),
    ("faqiu", "orb"),
    ("faqi", "off-hand"),
    ("danshoujian", "one-handed sword"),
    ("danshoufu", "one-handed axe"),
    ("danshoumao", "one-handed spear"),
    ("danshouchui", "one-handed mace"),
    ("danshounu", "one-handed crossbow"),
    ("liandao", "scythe"),
    ("lianchui", "flail"),
    ("juanzhou", "scroll"),
    // Cosmetics and the shop.
    ("shizhuang", "cosmetic"),
    ("sz", "cosmetic"),
    ("huanpi", "reskin"),
    ("shangcheng", "shop"),
    ("yueka", "monthly pass"),
    ("huodong", "event"),
    ("yunying", "live event"),
    ("zuoqi", "mount"),
    ("chongwu", "pet"),
    // Places and effects.
    ("chuansongmen", "portal"),
    ("dongxue", "cave"),
    ("dilao", "dungeon"),
    ("yaosai", "fortress"),
    ("huanggong", "palace"),
    ("fazhen", "magic circle"),
    ("huoyan", "flame"),
    ("shandian", "lightning"),
    ("jingti", "crystal"),
    ("xingkong", "starry sky"),
    ("youling", "ghost"),
    ("emo", "demon"),
    ("yanqiu", "eyeball"),
    ("yujing", "danger warning"),
    ("zhandou", "combat"),
    ("chuchang", "entrance"),
    ("wenshi", "ornament"),
    ("wenli", "pattern"),
    ("fujian", "attachment"),
    ("yiwu", "relic"),
    // Engine words.
    ("lod", "lower detail"),
];

/// Texture name endings and the map they hold.
const MAPS: &[(&str, &str)] = &[
    ("d", "colour map"),
    ("n", "normal map"),
    ("e", "glow map"),
    ("m", "mix map"),
];

/// English for the known words in a file name, joined by commas, or `None`
/// when it has none.
pub fn gloss(path: &str) -> Option<String> {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
    let tokens: Vec<String> = stem
        .split('_')
        .map(|t| {
            t.trim_end_matches(|c: char| c.is_ascii_digit())
                .to_ascii_lowercase()
        })
        .collect();
    let mut out: Vec<&str> = Vec::new();
    let mut add = |m: &'static str| {
        if !out.contains(&m) {
            out.push(m);
        }
    };
    for t in &tokens {
        if let Some((_, m)) = WORDS.iter().find(|(w, _)| w == t) {
            add(m);
        }
    }
    if ext.eq_ignore_ascii_case("texture2d") {
        if let Some((_, m)) = tokens
            .last()
            .and_then(|t| MAPS.iter().find(|(w, _)| w == t))
        {
            add(m);
        }
    }
    (!out.is_empty()).then(|| out.join(", "))
}
