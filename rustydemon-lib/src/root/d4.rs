use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::error::CascError;

// ── SNO group names and extensions ────────────────────────────────────────────

/// D4 SNO group ID → (folder name, file extension).
fn sno_group_info(group: i32) -> Option<(&'static str, &'static str)> {
    Some(match group {
        1 => ("Actor", ".acr"),
        2 => ("NPCComponentSet", ".npc"),
        3 => ("AIBehavior", ".aib"),
        4 => ("AIState", ".ais"),
        5 => ("AmbientSound", ".ams"),
        6 => ("Anim", ".ani"),
        7 => ("Anim2D", ".an2"),
        8 => ("AnimSet", ".ans"),
        9 => ("Appearance", ".app"),
        10 => ("Hero", ".hro"),
        11 => ("Cloth", ".clt"),
        12 => ("Conversation", ".cnv"),
        13 => ("ConversationList", ".cnl"),
        14 => ("EffectGroup", ".efg"),
        15 => ("Encounter", ".enc"),
        17 => ("Explosion", ".xpl"),
        18 => ("FlagSet", ".flg"),
        19 => ("Font", ".fnt"),
        20 => ("GameBalance", ".gam"),
        21 => ("Global", ".glo"),
        22 => ("LevelArea", ".lvl"),
        23 => ("Light", ".lit"),
        24 => ("MarkerSet", ".mrk"),
        26 => ("Observer", ".obs"),
        27 => ("Particle", ".prt"),
        28 => ("Physics", ".phy"),
        29 => ("Power", ".pow"),
        31 => ("Quest", ".qst"),
        32 => ("Rope", ".rop"),
        33 => ("Scene", ".scn"),
        35 => ("Script", ".scr"),
        36 => ("ShaderMap", ".shm"),
        37 => ("Shader", ".shd"),
        38 => ("Shake", ".shk"),
        39 => ("SkillKit", ".skl"),
        40 => ("Sound", ".snd"),
        42 => ("StringList", ".stl"),
        43 => ("Surface", ".srf"),
        44 => ("Texture", ".tex"),
        45 => ("Trail", ".trl"),
        46 => ("UI", ".ui"),
        47 => ("Weather", ".wth"),
        48 => ("World", ".wrl"),
        49 => ("Recipe", ".rcp"),
        51 => ("Condition", ".cnd"),
        52 => ("TreasureClass", ".trs"),
        53 => ("Account", ".acc"),
        57 => ("Material", ".mat"),
        59 => ("Lore", ".lor"),
        60 => ("Reverb", ".rev"),
        62 => ("Music", ".mus"),
        63 => ("Tutorial", ".tut"),
        67 => ("AnimTree", ".ant"),
        68 => ("Vibration", ".vib"),
        71 => ("wWiseSoundBank", ".wsb"),
        72 => ("Speaker", ".spk"),
        73 => ("Item", ".itm"),
        74 => ("PlayerClass", ".pcl"),
        76 => ("FogVolume", ".fog"),
        77 => ("Biome", ".bio"),
        78 => ("Wall", ".wal"),
        79 => ("SoundTable", ".sdt"),
        80 => ("Subzone", ".sbz"),
        81 => ("MaterialValue", ".mtv"),
        82 => ("MonsterFamily", ".mfm"),
        83 => ("TileSet", ".tst"),
        84 => ("Population", ".pop"),
        85 => ("MaterialValueSet", ".mvs"),
        86 => ("WorldState", ".wst"),
        87 => ("Schedule", ".sch"),
        88 => ("VectorField", ".vfd"),
        90 => ("Storyboard", ".stb"),
        92 => ("Territory", ".ter"),
        93 => ("AudioContext", ".auc"),
        94 => ("VOProcess", ".vop"),
        95 => ("DemonScroll", ".dss"),
        96 => ("QuestChain", ".qc"),
        97 => ("LoudnessPreset", ".lou"),
        98 => ("ItemType", ".itt"),
        99 => ("Achievement", ".ach"),
        100 => ("Crafter", ".crf"),
        101 => ("HoudiniParticlesSim", ".hps"),
        102 => ("Movie", ".vid"),
        103 => ("TiledStyle", ".tsl"),
        104 => ("Affix", ".aff"),
        105 => ("Reputation", ".rep"),
        106 => ("ParagonNode", ".pgn"),
        107 => ("MonsterAffix", ".maf"),
        108 => ("ParagonBoard", ".pbd"),
        109 => ("SetItemBonus", ".set"),
        110 => ("StoreProduct", ".prd"),
        111 => ("ParagonGlyph", ".gph"),
        112 => ("ParagonGlyphAffix", ".gaf"),
        114 => ("Challenge", ".cha"),
        115 => ("MarkingShape", ".msh"),
        116 => ("ItemRequirement", ".irq"),
        117 => ("Boost", ".bst"),
        118 => ("Emote", ".emo"),
        119 => ("Jewelry", ".jwl"),
        120 => ("PlayerTitle", ".pt"),
        121 => ("Emblem", ".emb"),
        122 => ("Dye", ".dye"),
        123 => ("FogOfWar", ".fow"),
        124 => ("ParagonThreshold", ".pth"),
        125 => ("AIAwareness", ".aia"),
        126 => ("TrackedReward", ".trd"),
        127 => ("CollisionSettings", ".col"),
        128 => ("Aspect", ".asp"),
        129 => ("ABTest", ".abt"),
        130 => ("Stagger", ".stg"),
        131 => ("EyeColor", ".eye"),
        132 => ("Makeup", ".mak"),
        133 => ("MarkingColor", ".mcl"),
        134 => ("HairColor", ".hcl"),
        135 => ("DungeonAffix", ".dax"),
        136 => ("Activity", ".act"),
        137 => ("Season", ".sea"),
        138 => ("HairStyle", ".har"),
        139 => ("FacialHair", ".fhr"),
        140 => ("Face", ".fac"),
        141 => ("MercenaryClass", ".mrc"),
        142 => ("PassivePowerContainer", ".ppc"),
        143 => ("MountProfile", ".mpp"),
        144 => ("AICoordinator", ".aic"),
        145 => ("CrafterTab", ".ctb"),
        146 => ("TownPortalCosmetic", ".tpc"),
        147 => ("AxeTest", ".axe"),
        148 => ("Wizard", ".wiz"),
        149 => ("FootstepTable", ".fst"),
        150 => ("Modal", ".mdl"),
        151 => ("CollectiblePower", ".cpw"),
        152 => ("AppearanceSet", ".aps"),
        153 => ("Preset", ".pst"),
        154 => ("PreviewComposition", ".pvc"),
        155 => ("SpawnPool", ".spn"),
        156 => ("Raid", ".rdx"),
        157 => ("BattlePassTier", ".bpt"),
        158 => ("Zone", ".zon"),
        159 => ("Unknown_159", ".ggu"),
        160 => ("DeathKit", ".dtk"),
        161 => ("Snippet", ".snp"),
        162 => ("CommunityModifier", ".cmo"),
        163 => ("GenericNodeGraph", ".gng"),
        164 => ("UserDefinedData", ".udd"),
        165 => ("DataStore", ".fds"),
        166 => ("BehaviorContainer", ".bvr"),
        167 => ("ActorService", ".asv"),
        168 => ("DamageRemap", ".dmg"),
        169 => ("Vendor", ".vnd"),
        170 => ("GenericSkillTree", ".gst"),
        171 => ("Unknown_171", ".dem"),
        172 => ("Crowd", ".crd"),
        173 => ("Unknown_173", ".crt"),
        174 => ("Unknown_174", ".crp"),
        175 => ("VisualRemap", ".vrm"),
        _ => return None,
    })
}

// ── SNO info ──────────────────────────────────────────────────────────────────

/// Resolved SNO entry from CoreTOC.dat.
#[derive(Debug, Clone)]
pub struct SnoInfo {
    pub group: i32,
    pub name: String,
    pub group_name: &'static str,
    pub ext: &'static str,
}

// ── CoreTOC parser ────────────────────────────────────────────────────────────

/// Parsed CoreTOC.dat: maps SNO ID → SnoInfo.
pub struct CoreToc {
    pub entries: HashMap<i32, SnoInfo>,
}

impl CoreToc {
    /// Parse CoreTOC.dat from raw bytes.
    pub fn parse(data: &[u8]) -> Result<Self, CascError> {
        let mut cursor = Cursor::new(data);
        let mut entries = HashMap::new();

        // Check for magic header (newer versions).
        let mut buf4 = [0u8; 4];
        cursor.read_exact(&mut buf4)?;
        let first = i32::from_le_bytes(buf4);

        let num_sno_groups;
        if first as u32 == 0xBCDE6611 {
            // New format with magic.
            cursor.read_exact(&mut buf4)?;
            num_sno_groups = i32::from_le_bytes(buf4) as usize;
        } else {
            // Old format: first i32 is the group count.
            num_sno_groups = first as usize;
        }

        if num_sno_groups > 300 {
            return Err(CascError::Config(format!(
                "CoreTOC: unreasonable group count {num_sno_groups}"
            )));
        }

        // Read entry counts per group.
        let mut entry_counts = vec![0i32; num_sno_groups];
        for c in &mut entry_counts {
            cursor.read_exact(&mut buf4)?;
            *c = i32::from_le_bytes(buf4);
        }

        // Read entry offsets per group.
        let mut entry_offsets = vec![0i32; num_sno_groups];
        for o in &mut entry_offsets {
            cursor.read_exact(&mut buf4)?;
            *o = i32::from_le_bytes(buf4);
        }

        // Read unknown counts per group.
        for _ in 0..num_sno_groups {
            cursor.read_exact(&mut buf4)?; // skip
        }

        // If magic format, also skip hash array.
        if first as u32 == 0xBCDE6611 {
            for _ in 0..num_sno_groups {
                cursor.read_exact(&mut buf4)?; // skip
            }
        }

        // Read unk1.
        cursor.read_exact(&mut buf4)?;

        // Compute header size for offset base.
        let header_size = if first as u32 == 0xBCDE6611 {
            4 + 4 + num_sno_groups * (4 + 4 + 4 + 4) + 4
        } else {
            4 + num_sno_groups * (4 + 4 + 4) + 4
        };

        // Parse each group's entries.
        for i in 0..num_sno_groups {
            let count = entry_counts[i];
            if count <= 0 {
                continue;
            }

            let (group_name, ext) = sno_group_info(i as i32).unwrap_or(("Unknown", ""));
            let base_offset = entry_offsets[i] as u64 + header_size as u64;

            cursor.seek(SeekFrom::Start(base_offset))?;

            // Read all fixed-size records: (group: i32, snoId: i32, nameOffset: i32) × count.
            let mut records = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let mut rec = [0u8; 12];
                cursor.read_exact(&mut rec)?;
                let sno_group = i32::from_le_bytes(rec[0..4].try_into().unwrap());
                let sno_id = i32::from_le_bytes(rec[4..8].try_into().unwrap());
                let name_offset = i32::from_le_bytes(rec[8..12].try_into().unwrap());
                records.push((sno_group, sno_id, name_offset));
            }

            // Name strings start immediately after the records.
            let names_base = base_offset + 12 * count as u64;

            for (sno_group, sno_id, name_offset) in records {
                cursor.seek(SeekFrom::Start(names_base + name_offset as u64))?;
                let name = read_cstring(&mut cursor)?;

                let (gn, ex) = sno_group_info(sno_group).unwrap_or((group_name, ext));

                entries.insert(
                    sno_id,
                    SnoInfo {
                        group: sno_group,
                        name,
                        group_name: gn,
                        ext: ex,
                    },
                );
            }
        }

        Ok(CoreToc { entries })
    }

    pub fn get(&self, sno_id: i32) -> Option<&SnoInfo> {
        self.entries.get(&sno_id)
    }
}

fn read_cstring(cursor: &mut Cursor<&[u8]>) -> Result<String, CascError> {
    let mut bytes = Vec::new();
    let mut b = [0u8];
    loop {
        if cursor.read_exact(&mut b).is_err() {
            break;
        }
        if b[0] == 0 {
            break;
        }
        bytes.push(b[0]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
