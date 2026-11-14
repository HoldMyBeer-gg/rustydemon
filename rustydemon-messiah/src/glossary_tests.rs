use super::glossary::gloss;

#[test]
fn armour_pieces_read_in_english() {
    assert_eq!(
        gloss("Mesh_m_crusader_toukui_half_sz06_009.mesh").as_deref(),
        Some("helmet, cosmetic")
    );
    assert_eq!(
        gloss("Mesh_m_crusader_jianjia_t04_001.mesh").as_deref(),
        Some("shoulder armour")
    );
    assert_eq!(
        gloss("m_crusader_yifu_tui_t03.material").as_deref(),
        Some("body armour, legs")
    );
}

#[test]
fn texture_suffixes_name_the_map() {
    assert_eq!(
        gloss("warlock_yuanchemo_01_d.texture2d").as_deref(),
        Some("colour map")
    );
    assert_eq!(
        gloss("warlock_yuanchemo_01_n.texture2d").as_deref(),
        Some("normal map")
    );
}

#[test]
fn names_without_known_words_get_nothing() {
    assert_eq!(
        gloss("Mesh_dun_sz03_003_lod1.mesh").as_deref(),
        Some("shield, cosmetic, lower detail")
    );
    assert_eq!(gloss("a_drink.anim"), None);
    assert_eq!(gloss("Interface/Icons/INV_Sword_04.blp"), None);
}

#[test]
fn a_word_is_said_once() {
    assert_eq!(gloss("toukui_toukui_all.mesh").as_deref(), Some("helmet"));
}
