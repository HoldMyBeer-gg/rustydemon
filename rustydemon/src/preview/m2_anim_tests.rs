use super::m2_anim::{anim_files, clips, sequences, BoneKeys, Sequence};
use super::m2_lite::tests::{bone, chunk, le, md20, put};
use super::m2_lite::{F_ANIMATIONS, F_BONES};

/// Append `bytes` to `b` and return where they start.
fn append(b: &mut Vec<u8>, bytes: &[u8]) -> u32 {
    let at = b.len() as u32;
    b.extend_from_slice(bytes);
    at
}

fn u32s(v: &[u32]) -> Vec<u8> {
    le(v)
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn i16s(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A 64-byte sequence record.
fn sequence(id: u16, variation: u16, duration: u32, flags: u32, alias_next: u16) -> Vec<u8> {
    let mut s = Vec::with_capacity(64);
    s.extend_from_slice(&id.to_le_bytes());
    s.extend_from_slice(&variation.to_le_bytes());
    s.extend_from_slice(&duration.to_le_bytes());
    s.extend_from_slice(&1.0f32.to_le_bytes()); // move speed
    s.extend_from_slice(&flags.to_le_bytes());
    s.extend_from_slice(&[0u8; 0x20 - 0x10]); // frequency, padding, replay, blend times
    s.extend_from_slice(&[0u8; 28]); // bounds
    s.extend_from_slice(&(-1i16).to_le_bytes()); // variation next
    s.extend_from_slice(&alias_next.to_le_bytes());
    assert_eq!(s.len(), 64);
    s
}

/// A 20-byte track header: interpolation, global sequence, and the two
/// outer arrays (one entry per sequence) at `times` and `values`.
fn track(global: i16, count: u32, times: u32, values: u32) -> Vec<u8> {
    let mut t = Vec::with_capacity(20);
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&global.to_le_bytes());
    t.extend_from_slice(&u32s(&[count, times, count, values]));
    t
}

/// The outer array entries for sequences 0 to 2: `(count, offset)` each.
fn outer(entries: [(u32, u32); 3]) -> Vec<u8> {
    u32s(
        &entries
            .iter()
            .flat_map(|(n, o)| [*n, *o])
            .collect::<Vec<_>>(),
    )
}

/// Three sequences (one in the model, one in an .anim file, one alias of
/// the first), two bones.  Bone 0 translates in sequence 0 from the model
/// and in sequence 1 from the .anim file; its rotation runs on a global
/// sequence and its scale has no keys.  Bone 1 only rotates, in
/// sequence 0.  Returns the model and the .anim file for sequence 1.
fn rigged_with_animation() -> (Vec<u8>, Vec<u8>) {
    let mut b = md20(274);
    let mut seqs = sequence(60, 0, 1000, 0x20, 0);
    seqs.extend(sequence(60, 1, 500, 0x81, 0));
    seqs.extend(sequence(60, 2, 1000, 0x40, 7));
    put(&mut b, F_ANIMATIONS, 3, &seqs);

    // Sequence 1 lives in the .anim file: keys at known offsets there.
    let mut anim = vec![0u8; 16];
    let a_times = append(&mut anim, &u32s(&[0, 500]));
    let a_vals = append(&mut anim, &f32s(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0]));

    // Bone 0 translation: sequence 0 in the model, sequence 1 in the file.
    let t0 = append(&mut b, &u32s(&[0, 333, 1000]));
    let v0 = append(
        &mut b,
        &f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 2.0]),
    );
    let times0 = append(&mut b, &outer([(3, t0), (2, a_times), (0, 0)]));
    let vals0 = append(&mut b, &outer([(3, v0), (2, a_vals), (0, 0)]));
    // Bone 0 rotation on global sequence 0: must be skipped.
    let gt = append(&mut b, &u32s(&[0]));
    let gv = append(&mut b, &i16s(&[32767, 32767, 32767, -1]));
    let gtimes = append(&mut b, &outer([(1, gt), (0, 0), (0, 0)]));
    let gvals = append(&mut b, &outer([(1, gv), (0, 0), (0, 0)]));
    // Bone 1 rotation: a quarter turn about Z at 250 ms, identity before.
    let rt = append(&mut b, &u32s(&[0, 250]));
    let rv = append(
        &mut b,
        &i16s(&[32767, 32767, 32767, -1, 32767, 32767, -9598, -9598]),
    );
    let rtimes = append(&mut b, &outer([(2, rt), (0, 0), (0, 0)]));
    let rvals = append(&mut b, &outer([(2, rv), (0, 0), (0, 0)]));

    let mut bones = bone(0, 0, -1, [0.0, 0.0, 1.0]);
    bones[0x10..0x24].copy_from_slice(&track(-1, 3, times0, vals0));
    bones[0x24..0x38].copy_from_slice(&track(0, 3, gtimes, gvals));
    bones.extend(bone(-1, 0, 0, [0.0, 0.0, 2.5]));
    bones[88 + 0x24..88 + 0x38].copy_from_slice(&track(-1, 3, rtimes, rvals));
    put(&mut b, F_BONES, 2, &bones);

    let mut f = chunk(b"MD21", &b);
    f.extend(chunk(
        b"AFID",
        &u32s(&[60 | (1 << 16), 4242, 60 | (2 << 16), 4343]),
    ));
    (f, anim)
}

#[test]
fn reads_the_sequence_table() {
    let (m2, _) = rigged_with_animation();
    let seqs = sequences(&m2).unwrap();
    assert_eq!(
        seqs[1],
        Sequence {
            id: 60,
            variation: 1,
            duration_ms: 500,
            flags: 0x81,
            alias_next: 0,
        }
    );
    assert!(seqs[0].stored_in_model());
    assert!(!seqs[1].stored_in_model());
    assert!(seqs[2].is_alias());
    assert_eq!(seqs[2].alias_next, 7);
    assert_eq!(seqs[0].name(), "0060-00");
}

#[test]
fn an_alias_stored_in_the_model_keeps_its_own_keys() {
    let (mut m2, _) = rigged_with_animation();
    // Flag the third sequence as stored in the model as well as an alias.
    let body_at = 8; // after the MD21 chunk header
    let table = u32::from_le_bytes(
        m2[body_at + F_ANIMATIONS + 4..body_at + F_ANIMATIONS + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    let flags_at = body_at + table + 2 * 64 + 0x0C;
    m2[flags_at..flags_at + 4].copy_from_slice(&0x60u32.to_le_bytes());
    let clips = clips(&m2, &|_, _| None).unwrap();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[1].sequence.name(), "0060-02");
    assert!(clips[1].sequence.is_alias());
}

#[test]
fn reads_the_anim_file_ids() {
    let (m2, _) = rigged_with_animation();
    assert_eq!(anim_files(&m2), [(60, 1, 4242), (60, 2, 4343)]);
}

#[test]
fn reads_keys_from_the_model_and_the_anim_file() {
    let (m2, anim) = rigged_with_animation();
    let clips = clips(&m2, &|id, variation| {
        (id == 60 && variation == 1).then(|| anim.clone())
    })
    .unwrap();
    assert_eq!(clips.len(), 2, "the alias has no keys of its own");
    assert_eq!(clips[0].sequence.name(), "0060-00");
    assert_eq!(
        clips[0].bones[0],
        BoneKeys {
            translation: vec![
                (0, [0.0, 0.0, 0.0]),
                (333, [1.0, 0.0, 0.0]),
                (1000, [0.0, 0.0, 2.0]),
            ],
            rotation: vec![],
            scale: vec![],
        }
    );
    assert_eq!(clips[1].sequence.name(), "0060-01");
    assert_eq!(
        clips[1].bones[0].translation,
        [(0, [0.0, 0.0, 0.0]), (500, [0.0, 1.0, 0.0])]
    );
    assert!(clips[1].bones[1].rotation.is_empty());
}

#[test]
fn decodes_compressed_quaternions() {
    let (m2, anim) = rigged_with_animation();
    let clips = clips(&m2, &|_, _| Some(anim.clone())).unwrap();
    let rot = &clips[0].bones[1].rotation;
    assert_eq!(rot[0], (0, [0.0, 0.0, 0.0, 1.0]));
    let (t, q) = rot[1];
    assert_eq!(t, 250);
    let half = std::f32::consts::FRAC_1_SQRT_2;
    for (got, want) in q.iter().zip([0.0, 0.0, half, half]) {
        assert!((got - want).abs() < 0.001, "{q:?}");
    }
}

#[test]
fn a_sequence_whose_anim_file_is_missing_is_left_out() {
    let (m2, _) = rigged_with_animation();
    let clips = clips(&m2, &|_, _| None).unwrap();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].sequence.name(), "0060-00");
}

#[test]
fn an_anim_file_wrapped_in_a_chunk_reads_the_same() {
    let (m2, anim) = rigged_with_animation();
    let wrapped = chunk(b"AFM2", &anim);
    let clips = clips(&m2, &|_, _| Some(wrapped.clone())).unwrap();
    assert_eq!(clips[1].bones[0].translation.len(), 2);
}

#[test]
fn keys_past_the_end_of_their_blob_are_dropped_not_fatal() {
    let (m2, anim) = rigged_with_animation();
    let short = anim[..anim.len() - 4].to_vec();
    let clips = clips(&m2, &|_, _| Some(short.clone())).unwrap();
    assert!(clips[1].bones[0].translation.is_empty());
    assert_eq!(clips[0].bones[0].translation.len(), 3);
}

#[test]
fn a_model_with_no_sequence_table_has_no_clips() {
    let plain = chunk(b"MD21", &md20(274));
    assert!(sequences(&plain).unwrap().is_empty());
    assert!(clips(&plain, &|_, _| None).unwrap().is_empty());
    assert!(anim_files(&plain).is_empty());
}
