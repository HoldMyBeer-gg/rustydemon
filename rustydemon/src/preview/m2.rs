//! M2 model preview plugin.
//!
//! The model itself is read by [`M2Lite`]; the skin file by wow-alchemy-m2.
//!
//! Renders the static bind pose with per-submesh textures loaded from
//! the TXID chunk's FileDataIDs.  The texture lookup chain is:
//!
//! ```text
//! skin.texture_units[i].texture_combo_index
//!   → md20.texture_lookup_table[combo_index]
//!   → texture_fdids[lookup_value]
//!   → BLP file (fetched via SiblingFetcher::by_fdid)
//! ```
//!
//! No animation, no doodad placement: those layer on later.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use wow_alchemy_data::types::VWowStructR;
use wow_alchemy_m2::skin::SkinVersion;
use wow_alchemy_m2::Skin;

use super::character;
use super::geosets;
use super::m2_lite::M2Lite;

use super::{
    Blend, ExportAction, Mesh3dCpu, MeshBatch, MeshMaterial, PreviewOutput, PreviewPlugin,
};

pub struct M2Preview;

/// Which monster skin (0 to 2) a texture slot of this type wears, if any.
fn monster_skin_slot(texture_type: Option<u32>) -> Option<usize> {
    match texture_type? {
        t @ 11..=13 => Some(t as usize - 11),
        _ => None,
    }
}

/// How the viewport draws a material with this M2 blend mode.  Modes it
/// does not know, and batches with no material, draw solid.
fn blend_for(mode: Option<u16>) -> Blend {
    match mode {
        Some(1) => Blend::AlphaKey,
        Some(2 | 7) => Blend::Alpha,
        Some(3) => Blend::Add,
        Some(4) => Blend::AddAlpha,
        Some(5) => Blend::Mod,
        Some(6) => Blend::Mod2x,
        _ => Blend::Opaque,
    }
}

/// Texture type of the slot an item's model leaves for the item's skin.
const ITEM_SKIN: u32 = 2;

/// A model's geometry, ready to draw or to join to another's.
struct Part {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    batches: Vec<MeshBatch>,
    materials: Vec<MeshMaterial>,
}

impl Part {
    /// Join `other` to this one, moved so its origin sits at `at`.
    fn hang(&mut self, other: Part, at: [f32; 3]) {
        let vertex = self.positions.len() as u32;
        let index = self.indices.len() as u32;
        let material = self.materials.len() as u32;
        self.positions.extend(
            other
                .positions
                .iter()
                .map(|p| [p[0] + at[0], p[1] + at[1], p[2] + at[2]]),
        );
        self.uvs.extend(other.uvs);
        self.indices
            .extend(other.indices.iter().map(|i| i + vertex));
        self.batches.extend(other.batches.iter().map(|b| MeshBatch {
            start_index: b.start_index + index,
            material_id: b.material_id + material,
            ..*b
        }));
        self.materials.extend(other.materials);
    }
}

/// The file a texture slot of a worn model shows: the one the model names,
/// else the item's own skin in the slot made for it.
fn worn_texture(named: u32, slot_type: u32, item_texture: u32) -> u32 {
    match (named, slot_type) {
        (0, ITEM_SKIN) => item_texture,
        _ => named,
    }
}

/// The draws of a skin, one per base texture unit.  `visible` says which
/// sections to keep; `None` keeps them all.
fn batches_of(
    skin: &Skin,
    m2: &M2Lite,
    material_count: usize,
    visible: Option<&[bool]>,
) -> Vec<MeshBatch> {
    if skin.texture_units.is_empty() {
        // Fallback: one batch per submesh (legacy models).
        return skin
            .submeshes
            .iter()
            .enumerate()
            .map(|(i, s)| MeshBatch {
                start_index: geosets::first_index(s.triangle_start, s.level),
                index_count: s.triangle_count as u32,
                material_id: i as u32,
                blend: Blend::Opaque,
            })
            .collect();
    }
    skin.texture_units
        .iter()
        .filter(|tu| tu.material_layer == 0)
        .filter(|tu| {
            visible
                .and_then(|v| v.get(tu.skin_section_index as usize))
                .copied()
                .unwrap_or(true)
        })
        .filter_map(|tu| {
            let sub = skin.submeshes.get(tu.skin_section_index as usize)?;
            // Resolve texture: skip combo_index entries in the lookup
            // table, take texture_count, first is diffuse.
            let combo_idx = tu.texture_combo_index as usize;
            let tex_idx = m2.texture_lookup.get(combo_idx).copied().unwrap_or(-1);
            let material_id = if tex_idx >= 0 && (tex_idx as usize) < material_count {
                tex_idx as u32
            } else {
                tu.skin_section_index as u32
            };
            Some(MeshBatch {
                start_index: geosets::first_index(sub.triangle_start, sub.level),
                index_count: sub.triangle_count as u32,
                material_id,
                blend: blend_for(m2.blend_modes.get(tu.material_index as usize).copied()),
            })
        })
        .collect()
}

/// The geometry of a model a character wears, textured with the item's
/// skin.  The error says what could not be read.
fn worn_part(
    piece: &super::gear::Attached,
    fetch: &dyn Fn(u32) -> Option<Vec<u8>>,
) -> Result<Part, String> {
    let missing = |what: &str, file: u32| format!("{what} {file} is not in this install");
    let bytes = fetch(piece.model).ok_or_else(|| missing("file", piece.model))?;
    let m2 = M2Lite::parse(&bytes)?;
    let skin_id = *m2.skin_fdids.first().ok_or("it lists no skin file")?;
    let skin_bytes = fetch(skin_id).ok_or_else(|| missing("skin file", skin_id))?;
    let skin = Skin::wow_read(&mut Cursor::new(skin_bytes.as_slice()), SkinVersion::V3)
        .map_err(|e| format!("its skin could not be read: {e}"))?;
    let materials: Vec<MeshMaterial> = m2
        .texture_types
        .iter()
        .enumerate()
        .map(|(slot, &kind)| {
            let named = m2.texture_fdids.get(slot).copied().unwrap_or(0);
            let file = worn_texture(named, kind, piece.texture);
            let decoded = (file != 0)
                .then(|| fetch(file))
                .flatten()
                .as_deref()
                .and_then(decode_blp);
            match decoded {
                Some((rgba, width, height)) => MeshMaterial {
                    rgba: Some(rgba),
                    width,
                    height,
                },
                None => MeshMaterial {
                    rgba: None,
                    width: 1,
                    height: 1,
                },
            }
        })
        .collect();
    Ok(Part {
        indices: skin
            .triangles
            .iter()
            .map(|&i| skin.indices.get(i as usize).copied().unwrap_or(0) as u32)
            .collect(),
        batches: batches_of(&skin, &m2, materials.len(), None),
        positions: m2.positions,
        uvs: m2.uvs,
        materials,
    })
}

const NO_MESH: &str = "\n\nThis model has no mesh to draw. Spell effects made only of \
                       particles and camera paths are stored as M2 files too.";

fn looks_like_m2(data: &[u8]) -> bool {
    data.len() >= 4 && (&data[..4] == b"MD20" || &data[..4] == b"MD21")
}

/// Decode a BLP byte buffer into RGBA8 + dimensions.
fn decode_blp(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let blp = rustydemon_blp2::BlpFile::from_bytes(bytes.to_vec()).ok()?;
    let (pixels, w, h) = blp.get_pixels(0).ok()?;
    Some((pixels, w, h))
}

impl PreviewPlugin for M2Preview {
    fn name(&self) -> &str {
        ".m2 (WoW model)"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        if !filename.to_ascii_lowercase().ends_with(".m2") {
            return false;
        }
        looks_like_m2(data)
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        siblings: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        let m2 = match M2Lite::parse(data) {
            Ok(m) => m,
            Err(e) => {
                out.text = Some(format!("M2 parse failed: {e}"));
                return out;
            }
        };
        let is_chunked = m2.chunked;
        let skin_fdids = &m2.skin_fdids;
        let texture_fdids = &m2.texture_fdids;

        let mut text = format!(
            "M2 model  •  {filename}\n\
             ──────────────────────────\n\
             format    : {}\n\
             name      : {}\n\
             vertices  : {}\n\
             textures  : {}\n\
             materials : {}\n\
             bones     : {}\n\
             skin files: {}",
            if is_chunked {
                "MD21 (Legion+)"
            } else {
                "MD20 (legacy)"
            },
            m2.name,
            m2.positions.len(),
            m2.texture_count,
            m2.material_count,
            m2.bone_count,
            skin_fdids.len(),
        );

        if m2.positions.is_empty() {
            text.push_str(NO_MESH);
            out.text = Some(text);
            return out;
        }

        // ── Pull SKIN 0 (highest-quality LOD) via the FDID fetcher ───────────
        let Some(&skin_id) = skin_fdids.first() else {
            text.push_str(
                "\n\nThis model lists no skin file, so there are no triangles \
                 to draw. Skins stored inside the model are not read yet.",
            );
            out.text = Some(text);
            return out;
        };
        let Some(skin_bytes) = (siblings.by_fdid)(skin_id) else {
            text.push_str(&format!(
                "\n\nCould not read skin file {skin_id}, which holds this model's \
                 triangles. It may not be downloaded in this install.",
            ));
            out.text = Some(text);
            return out;
        };

        let mut skin_reader = Cursor::new(skin_bytes.as_slice());
        let skin = match Skin::wow_read(&mut skin_reader, SkinVersion::V3) {
            Ok(s) => s,
            Err(e) => {
                text.push_str(&format!("\n\nSKIN parse failed: {e}"));
                out.text = Some(text);
                return out;
            }
        };

        // ── Build the CPU-side mesh ──────────────────────────────────────────
        let positions = m2.positions.clone();
        let uvs = m2.uvs.clone();

        // Skin triangles index into `skin.indices` (the vertex lookup
        // table), which in turn indexes into the MD20 vertex array.
        let indices: Vec<u32> = skin
            .triangles
            .iter()
            .map(|&i| skin.indices.get(i as usize).copied().unwrap_or(0) as u32)
            .collect();

        if positions.is_empty() || indices.is_empty() {
            text.push_str(NO_MESH);
            out.text = Some(text);
            return out;
        }

        // ── Fetch and decode textures by FDID ────────────────────────────────
        // Build one MeshMaterial per TXID entry.  The texture_lookup_table
        // and skin texture_units will map submeshes → material slots.
        let mut materials: Vec<MeshMaterial> = Vec::with_capacity(texture_fdids.len());
        let mut tex_loaded: u32 = 0;
        let mut tex_failed: u32 = 0;
        // Texture slots the file leaves empty for a creature's skin are
        // filled from the game's creature tables.
        let monster_skins = (siblings.monster_skins)();
        let mut skinned: u32 = 0;
        // A character body's empty slots are painted from the choices the
        // character creation screen starts with.
        let (look, look_problem) = match (siblings.character)() {
            Ok(look) => (look, None),
            Err(problem) => (None, Some(problem)),
        };
        let decode = |fdid: u32| (siblings.by_fdid)(fdid).as_deref().and_then(decode_blp);
        let mut painted: Vec<String> = Vec::new();
        for (slot, &fdid) in texture_fdids.iter().enumerate() {
            let sheet = look
                .as_ref()
                .filter(|_| fdid == 0)
                .and_then(|look| {
                    let kind = *m2.texture_types.get(slot)?;
                    look.sheets.iter().find(|s| s.texture_type == kind)
                })
                .and_then(|plan| Some((plan, character::compose(plan, &decode)?)));
            if let Some((plan, (rgba, width, height))) = sheet {
                painted.push(format!(
                    "slot {slot}: {width}x{height} from {} texture(s)",
                    plan.paints.len()
                ));
                tex_loaded += 1;
                materials.push(MeshMaterial {
                    rgba: Some(rgba),
                    width,
                    height,
                });
                continue;
            }
            let fdid = if fdid != 0 {
                fdid
            } else {
                let skin = monster_skin_slot(m2.texture_types.get(slot).copied())
                    .and_then(|i| monster_skins.map(|s| s[i]))
                    .unwrap_or(0);
                skinned += u32::from(skin != 0);
                skin
            };
            if fdid == 0 {
                // Supplied by the game at runtime (character skin, hair,
                // or a creature skin we could not look up).
                tex_failed += 1;
                materials.push(MeshMaterial {
                    rgba: None,
                    width: 1,
                    height: 1,
                });
                continue;
            }
            let decoded = (siblings.by_fdid)(fdid).as_deref().and_then(decode_blp);
            match decoded {
                Some((rgba, w, h)) => {
                    tex_loaded += 1;
                    materials.push(MeshMaterial {
                        rgba: Some(rgba),
                        width: w,
                        height: h,
                    });
                }
                None => {
                    tex_failed += 1;
                    materials.push(MeshMaterial {
                        rgba: None,
                        width: 1,
                        height: 1,
                    });
                }
            }
        }

        // ── Build render batches from skin texture_units ─────────────────────
        // Following Everlook's pattern: iterate texture_units (render
        // batches), not submeshes.  Each texture_unit references a submesh
        // via `skin_section_index` for geometry and resolves its texture
        // via `texture_combo_index` + `texture_count` into the
        // `texture_lookup_table`.  The first lookup entry is the diffuse
        // texture.
        //
        // Multiple texture_units can reference the same submesh (diffuse
        // pass, specular pass, env map pass).  For our single-texture
        // renderer we take `material_layer == 0` as the base/diffuse pass
        // and skip higher layers.
        // A character body holds every glove, boot and hairstyle at once;
        // draw the undressed set, not all of them on top of each other.
        let section_ids: Vec<u16> = skin.submeshes.iter().map(|s| s.id).collect();
        let character = geosets::is_character(&m2.texture_types);
        let visible = if let Some(look) = &look {
            geosets::chosen(&section_ids, &look.geosets)
        } else if character {
            geosets::undressed(&section_ids)
        } else {
            vec![true; section_ids.len()]
        };
        let batches = batches_of(&skin, &m2, materials.len(), Some(&visible));

        // What the character wears as models of their own: a helm, a pair
        // of shoulder pads.  Each hangs where the body's skeleton says.
        let mut body = Part {
            positions,
            uvs,
            indices,
            batches,
            materials,
        };
        let own = (body.indices.len(), body.batches.len(), body.materials.len());
        let mut worn: Vec<String> = Vec::new();
        for piece in look.iter().flat_map(|l| &l.attached) {
            let Some(at) = m2.attachment(piece.point) else {
                worn.push(format!(
                    "model {} not drawn: the body has no attachment point {}",
                    piece.model, piece.point
                ));
                continue;
            };
            match worn_part(piece, siblings.by_fdid) {
                Ok(part) => {
                    worn.push(format!(
                        "model {} on attachment point {}: {} vertices",
                        piece.model,
                        piece.point,
                        part.positions.len()
                    ));
                    body.hang(part, at);
                }
                Err(problem) => worn.push(format!("model {} not drawn: {problem}", piece.model)),
            }
        }
        let Part {
            positions,
            uvs,
            indices,
            batches,
            materials,
        } = body;

        let (mn, mx) = compute_bbox(&positions);
        let mesh = Arc::new(Mesh3dCpu {
            positions,
            uvs,
            indices,
            bbox_min: mn,
            bbox_max: mx,
            batches,
            materials,
        });

        // OBJ export captures the resolved mesh so the skin indirection
        // and texture mapping are already baked in.
        let mesh_for_obj = Arc::clone(&mesh);
        out.extra_exports.push(ExportAction {
            label: "Export As OBJ",
            default_extension: "obj",
            filter_name: "Wavefront OBJ",
            build: Arc::new(move |_raw, _path| Ok(super::encode_obj(&mesh_for_obj))),
        });

        // OBJ + MTL + texture PNGs: writes sibling files next to the OBJ.
        if !mesh.materials.is_empty() {
            let mesh_for_mtl = Arc::clone(&mesh);
            out.extra_exports.push(ExportAction {
                label: "Export As OBJ + MTL",
                default_extension: "obj",
                filter_name: "Wavefront OBJ",
                build: Arc::new(move |_raw, path| {
                    super::encode_obj_with_materials(&mesh_for_mtl, path)
                }),
            });
        }

        // Rigged glTF: the skeleton from the model, the parts from the
        // batches drawn above, the textures as PNG, and every sequence as
        // an animation.  Sequences kept in .anim files are read now, while
        // the archive is at hand; a model's set of them is small.
        let sequences = super::m2_anim::sequences(data).unwrap_or_default();
        let wanted: Vec<(u16, u16)> = sequences
            .iter()
            .filter(|s| !s.stored_in_model() && !s.is_alias())
            .map(|s| (s.id, s.variation))
            .collect();
        let anim_files: HashMap<(u16, u16), Vec<u8>> = super::m2_anim::anim_files(data)
            .into_iter()
            .filter(|(id, variation, fdid)| *fdid != 0 && wanted.contains(&(*id, *variation)))
            .filter_map(|(id, variation, fdid)| Some(((id, variation), (siblings.by_fdid)(fdid)?)))
            .collect();
        let external = wanted.len();
        let missing = wanted
            .iter()
            .filter(|k| !anim_files.contains_key(k))
            .count();
        if !m2.bones.is_empty() {
            text.push_str(&format!(
                "\nanimations: {} sequences, {} in .anim files",
                sequences.iter().filter(|s| !s.is_alias()).count(),
                external
            ));
            if missing > 0 {
                text.push_str(&format!(" ({missing} not in this install)"));
            }
        }
        let anim_files = Arc::new(anim_files);
        let mesh_for_glb = Arc::clone(&mesh);
        out.extra_exports.push(ExportAction {
            label: "Export As glTF (rigged)",
            default_extension: "glb",
            filter_name: "glTF binary",
            build: Arc::new(move |raw, _path| {
                let m2 = M2Lite::parse(raw)?;
                let mut textures = Vec::new();
                // The rig is the body's; what it wears stays out of it.
                let (indices, batches, materials) = own;
                for (slot, material) in mesh_for_glb.materials[..materials].iter().enumerate() {
                    if let Some(rgba) = &material.rgba {
                        textures.push((
                            super::m2_rig::texture_name(slot),
                            super::encode_png(rgba, material.width, material.height)?,
                        ));
                    }
                }
                let mut model = super::m2_rig::model(
                    &m2,
                    &mesh_for_glb.indices[..indices],
                    &mesh_for_glb.batches[..batches],
                    textures,
                );
                let clips = super::m2_anim::clips(raw, &|id, variation| {
                    anim_files.get(&(id, variation)).cloned()
                })?;
                model.animations = super::m2_rig::animations(&m2, &clips);
                Ok(super::gltf::to_glb(&model))
            }),
        });

        let batch_count = mesh.batches.len();
        out.mesh3d = Some(mesh);

        // Debug: show texture_unit → texture mapping.
        text.push_str("\n\nTexture unit mapping:");
        for (i, tu) in skin.texture_units.iter().enumerate() {
            let combo_idx = tu.texture_combo_index as usize;
            let tex_idx = m2.texture_lookup.get(combo_idx).copied().unwrap_or(-1);
            text.push_str(&format!(
                "\n  [{}] section={} layer={} combo_idx={} tex_count={} → tex_idx={} shader=0x{:04X}",
                i,
                tu.skin_section_index,
                tu.material_layer,
                tu.texture_combo_index,
                tu.texture_count,
                tex_idx,
                tu.shader_id,
            ));
        }

        if !texture_fdids.is_empty() {
            text.push_str(&format!(
                "\n\nTextures: {tex_loaded}/{} loaded{}",
                texture_fdids.len(),
                if tex_failed > 0 {
                    format!(" ({tex_failed} fallback)")
                } else {
                    String::new()
                },
            ));
            if skinned > 0 {
                text.push_str(&format!(
                    "\n{skinned} from the creature's first display (other displays use other skins)"
                ));
            }
        }
        if let Some(problem) = &look_problem {
            text.push_str(&format!("\nCharacter look not applied: {problem}"));
        }
        for note in look.iter().flat_map(|l| &l.notes) {
            text.push_str(&format!("\n{note}"));
        }
        for line in &worn {
            text.push_str(&format!("\nWorn {line}"));
        }
        if !painted.is_empty() {
            text.push_str(&format!(
                "\nCharacter creation defaults painted: {}",
                painted.join("; ")
            ));
        }
        if character || look.is_some() {
            let mut shown: Vec<u16> = section_ids
                .iter()
                .zip(&visible)
                .filter_map(|(&id, &on)| on.then_some(id))
                .collect();
            shown.sort_unstable();
            shown.dedup();
            let list: Vec<String> = shown.iter().map(u16::to_string).collect();
            text.push_str(&format!(
                "\nCharacter body. Parts drawn: {}",
                list.join(", ")
            ));
        }
        text.push_str(&format!(
            "\nLoaded SKIN with {} submesh(es), {} render batch(es)",
            skin.submeshes.len(),
            batch_count
        ));
        out.text = Some(text);
        out
    }
}

fn compute_bbox(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in positions {
        for i in 0..3 {
            if p[i] < mn[i] {
                mn[i] = p[i];
            }
            if p[i] > mx[i] {
                mx[i] = p[i];
            }
        }
    }
    (mn, mx)
}

#[cfg(test)]
mod tests {
    use super::super::{Blend, SiblingFetcher};
    use super::*;

    #[test]
    fn material_blend_modes_map_to_how_the_viewport_draws() {
        assert_eq!(blend_for(Some(0)), Blend::Opaque);
        assert_eq!(blend_for(Some(1)), Blend::AlphaKey);
        assert_eq!(blend_for(Some(2)), Blend::Alpha);
        assert_eq!(blend_for(Some(3)), Blend::Add);
        assert_eq!(blend_for(Some(4)), Blend::AddAlpha);
        assert_eq!(blend_for(Some(5)), Blend::Mod);
        assert_eq!(blend_for(Some(6)), Blend::Mod2x);
        assert_eq!(blend_for(Some(7)), Blend::Alpha);
        // Unknown modes and missing materials draw solid, as before.
        assert_eq!(blend_for(Some(99)), Blend::Opaque);
        assert_eq!(blend_for(None), Blend::Opaque);
    }

    fn part(positions: Vec<[f32; 3]>, indices: Vec<u32>, material_id: u32) -> Part {
        Part {
            uvs: vec![[0.5, 0.5]; positions.len()],
            batches: vec![MeshBatch {
                start_index: 0,
                index_count: indices.len() as u32,
                material_id,
                blend: Blend::Opaque,
            }],
            materials: vec![MeshMaterial {
                rgba: None,
                width: 1,
                height: 1,
            }],
            positions,
            indices,
        }
    }

    #[test]
    fn a_worn_model_joins_the_body_at_its_attachment_point() {
        let corner = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut body = part(corner.to_vec(), vec![0, 1, 2], 0);
        let helm = part(corner.to_vec(), vec![2, 1, 0], 0);
        body.hang(helm, [0.0, 0.0, 1.75]);

        assert_eq!(body.positions.len(), 6);
        assert_eq!(body.positions[3], [0.0, 0.0, 1.75]);
        assert_eq!(body.positions[4], [1.0, 0.0, 1.75]);
        assert_eq!(body.uvs.len(), 6);
        // The helm's triangles point at its own vertices, after the body's.
        assert_eq!(body.indices, [0, 1, 2, 5, 4, 3]);
        let helm = body.batches[1];
        assert_eq!((helm.start_index, helm.index_count), (3, 3));
        // And at its own texture, after the body's.
        assert_eq!(helm.material_id, 1);
        assert_eq!(body.materials.len(), 2);
    }

    #[test]
    fn a_worn_models_empty_slot_takes_the_items_texture() {
        // A slot the file names keeps its own.
        assert_eq!(worn_texture(555, 2, 9000), 555);
        // The item's skin goes in the slot made for it.
        assert_eq!(worn_texture(0, 2, 9000), 9000);
        assert_eq!(worn_texture(0, 1, 9000), 0);
    }

    fn build_without_skin(model: &[u8]) -> String {
        let none = |_: u32| None;
        let no_name = |_: &str| None;
        let no_info = |_: &str| None;
        let no_skins = || None;
        let no_repo = || None;
        let siblings = SiblingFetcher {
            by_name: &no_name,
            by_fdid: &none,
            texture_info: &no_info,
            monster_skins: &no_skins,
            character: &|| Ok(None),
            messiah_repo: &no_repo,
        };
        M2Preview
            .build("crab.m2", model, &egui::Context::default(), &siblings)
            .text
            .unwrap()
    }

    #[test]
    fn a_listed_skin_that_cannot_be_read_is_named() {
        let text = build_without_skin(&super::super::m2_lite::tests::chunked());
        assert!(text.contains("skin file 111"), "{text}");
        assert!(!text.contains("legacy"), "{text}");
    }

    #[test]
    fn a_model_that_lists_no_skin_says_so() {
        let chunked = super::super::m2_lite::tests::chunked();
        // Keep only the MD21 chunk: no SFID.
        let len = u32::from_le_bytes(chunked[4..8].try_into().unwrap()) as usize;
        let text = build_without_skin(&chunked[..8 + len]);
        assert!(text.contains("lists no skin file"), "{text}");
    }
}
