//! Which parts of a character model to draw.
//!
//! A character's `.m2` holds every variant of every body part at once: bare
//! hands and three glove cuffs, bare feet and four boot tops, each hairstyle,
//! each robe skirt.  A part is a skin section, and its id says what it is:
//! `group * 100 + variant`, so 401 is bare hands and 402 a short glove cuff.
//! The game draws one variant per group, chosen by what the character wears.
//!
//! This picks the undressed set: the body, the first hairstyle, and the
//! first variant of every other group (the second for ears and face).

/// Texture type of the slot the game fills with the composited body skin.
/// Only character models have one.
const BODY_SKIN: u32 = 1;

/// Groups whose first variant is not the one to draw: variant 1 of the
/// ears is "no ears" and of the face a low-detail stub.  Both draw variant 2
/// when the model has it.
const SECOND_VARIANT_GROUPS: [u16; 2] = [7, 32];

/// Whether a model with these texture slot types is a character body.
pub fn is_character(texture_types: &[u32]) -> bool {
    texture_types.contains(&BODY_SKIN)
}

/// Where a skin section's triangles start in the skin's triangle list.
///
/// The file keeps this in 16 bits, with the overflow in the field that
/// follows the id, so a section past index 65,535 is only found by joining
/// the two.
pub fn first_index(triangle_start: u16, level: u16) -> u32 {
    (u32::from(level) << 16) | u32::from(triangle_start)
}

/// For each section id, whether an undressed character shows it.
pub fn undressed(ids: &[u16]) -> Vec<bool> {
    let hair = ids
        .iter()
        .copied()
        .filter(|&id| (1..100).contains(&id))
        .min();
    ids.iter()
        .map(|&id| match id {
            0 => true,
            1..=99 => Some(id) == hair,
            _ => {
                let group = id / 100;
                let second = group * 100 + 2;
                if SECOND_VARIANT_GROUPS.contains(&group) && ids.contains(&second) {
                    id == second
                } else {
                    id % 100 == 1
                }
            }
        })
        .collect()
}

/// [`undressed`], changed by the parts a character's choices pick.  Each
/// pick is `(group, variant)` and replaces whatever its group showed; a
/// variant the model does not have leaves the group empty.
pub fn chosen(ids: &[u16], picks: &[(u32, u32)]) -> Vec<bool> {
    let mut visible = undressed(ids);
    for &(group, variant) in picks {
        for (&id, on) in ids.iter().zip(&mut visible) {
            if id != 0 && u32::from(id / 100) == group {
                *on = u32::from(id) == group * 100 + variant;
            }
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(ids: &[u16]) -> Vec<u16> {
        let mut out: Vec<u16> = ids
            .iter()
            .zip(undressed(ids))
            .filter_map(|(&id, on)| on.then_some(id))
            .collect();
        out.dedup();
        out
    }

    fn picked(ids: &[u16], picks: &[(u32, u32)]) -> Vec<u16> {
        ids.iter()
            .zip(chosen(ids, picks))
            .filter_map(|(&id, on)| on.then_some(id))
            .collect()
    }

    #[test]
    fn a_choice_replaces_what_its_group_showed() {
        let ids = [0, 1, 2, 3, 401, 402, 701, 702, 703];
        // Hairstyle 3 and the third ear shape.
        assert_eq!(picked(&ids, &[(0, 3), (7, 3)]), [0, 3, 401, 703]);
        assert_eq!(picked(&ids, &[]), [0, 1, 401, 702], "no picks: undressed");
    }

    #[test]
    fn a_choice_of_a_variant_the_model_lacks_empties_the_group() {
        // "No hair", and a group whose variant 0 means nothing.
        assert_eq!(picked(&[0, 1, 2, 3501], &[(0, 0), (35, 0)]), [0]);
    }

    #[test]
    fn a_body_skin_slot_marks_a_character() {
        assert!(is_character(&[0, 1, 6]));
        // A creature: fixed textures and monster skins.
        assert!(!is_character(&[0, 11, 12]));
        assert!(!is_character(&[]));
    }

    #[test]
    fn a_section_past_the_16_bit_limit_starts_where_both_fields_say() {
        assert_eq!(first_index(120, 0), 120);
        assert_eq!(first_index(120, 1), 65_656);
        assert_eq!(first_index(0xFFFF, 2), 0x2_FFFF);
    }

    #[test]
    fn the_body_is_always_drawn_even_when_split_into_pieces() {
        assert_eq!(undressed(&[0, 0, 0]), [true, true, true]);
    }

    #[test]
    fn one_hairstyle_is_drawn_not_all_of_them() {
        assert_eq!(shown(&[0, 3, 4, 5]), [0, 3]);
    }

    #[test]
    fn each_group_draws_its_first_variant() {
        // Hands, feet, legs: bare ones, not every glove, boot and robe.
        assert_eq!(
            shown(&[0, 401, 402, 403, 501, 502, 1301, 1302, 2001, 2002]),
            [0, 401, 501, 1301, 2001]
        );
    }

    #[test]
    fn a_group_with_no_first_variant_draws_nothing() {
        // Facial features and eye glow start at variant 2; variant 1 means
        // none and has no geometry.
        assert_eq!(shown(&[0, 302, 303, 1702, 1703]), [0]);
    }

    #[test]
    fn ears_are_drawn_when_the_model_has_them() {
        assert_eq!(shown(&[0, 701, 702]), [0, 702]);
        assert_eq!(shown(&[0, 701]), [0, 701]);
    }

    /// Variant 1 of the face group is a stub of a few dozen triangles; the
    /// face itself is variant 2.
    #[test]
    fn the_face_is_drawn_not_its_stub() {
        assert_eq!(shown(&[0, 3201, 3202]), [0, 3202]);
        assert_eq!(shown(&[0, 3201]), [0, 3201]);
    }

    #[test]
    fn a_section_stored_twice_is_drawn_twice() {
        // The same id for two materials of one part.
        assert_eq!(undressed(&[1301, 1301, 1302]), [true, true, false]);
    }
}
