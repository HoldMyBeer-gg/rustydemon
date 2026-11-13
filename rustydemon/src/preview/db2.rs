//! A reader for the game's client database tables (`.db2`, format `WDC5`).
//!
//! A table is a header, one or more sections of fixed-size records, and
//! side tables that the records index into.  A record is a bit string; each
//! field says where its bits are and how to turn them into a value:
//!
//! * plain: the bits are the value (or several, for an array)
//! * bit-packed: a narrow unsigned or signed integer
//! * pallet: the bits are an index into a shared list of values (or of
//!   small arrays)
//! * common: the field is not in the record at all; a side table lists the
//!   rows that differ from a default
//!
//! This reads the fields the caller asks for and nothing else.  It does not
//! read strings, relationship data, or tables with variable-size records.
//! Every offset and count comes from the file and is checked before use.
//!
//! Sections can be encrypted with keys that are not public.  The file layer
//! hands those over as zeros (see `CascHandler::open_file_by_fdid_partial`);
//! a section with a key and nothing but zeros is skipped.

/// One row: its id, and the requested fields in the order asked for.  Each
/// field is a list because a field can be an array.  Signed fields are
/// sign-extended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: u32,
    pub fields: Vec<Vec<u64>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Identifies the column layout; it changes when Blizzard adds, removes
    /// or reorders columns.
    pub layout_hash: u32,
    /// How many columns the table has.
    pub field_count: usize,
    pub rows: Vec<Row>,
}

const HEADER_SIZE: usize = 204;
const SECTION_HEADER_SIZE: usize = 40;
const FIELD_STORAGE_SIZE: usize = 24;
/// Header flag: records vary in size and are found through an offset map.
const FLAG_VARIABLE_RECORDS: u16 = 1;

/// How a field's value is stored.
#[derive(Clone, Copy)]
enum Kind {
    Plain { element_bits: usize },
    Packed { signed: bool },
    Common { default: u32, at: usize, len: usize },
    Pallet { at: usize, len: usize },
    PalletArray { at: usize, len: usize, count: usize },
}

#[derive(Clone, Copy)]
struct Field {
    offset_bits: usize,
    size_bits: usize,
    kind: Kind,
}

/// Bounds-checked little-endian reads; `None` means the file is too short.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn slice(&self, at: usize, len: usize) -> Option<&'a [u8]> {
        self.0.get(at..at.checked_add(len)?)
    }
    fn u16(&self, at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.slice(at, 2)?.try_into().ok()?))
    }
    fn u32(&self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.slice(at, 4)?.try_into().ok()?))
    }
    fn u64(&self, at: usize) -> Option<u64> {
        Some(u64::from_le_bytes(self.slice(at, 8)?.try_into().ok()?))
    }
    fn size(&self, at: usize) -> Option<usize> {
        self.u32(at).map(|v| v as usize)
    }
}

/// `size` bits of `record` starting at bit `offset`, lowest bit first.
fn bits(record: &[u8], offset: usize, size: usize) -> Option<u64> {
    if size == 0 || size > 64 || offset.checked_add(size)? > record.len() * 8 {
        return None;
    }
    let mut value = 0u64;
    for i in 0..size {
        let bit = offset + i;
        if record[bit / 8] >> (bit % 8) & 1 == 1 {
            value |= 1 << i;
        }
    }
    Some(value)
}

impl Field {
    /// This field's value in `record`, the row with id `id`.
    fn read(&self, record: &[u8], id: u32, pallet: &Bytes, common: &Bytes) -> Option<Vec<u64>> {
        let index = || bits(record, self.offset_bits, self.size_bits).map(|v| v as usize);
        Some(match self.kind {
            Kind::Plain { element_bits } => (0..self.size_bits / element_bits)
                .map(|i| bits(record, self.offset_bits + i * element_bits, element_bits))
                .collect::<Option<_>>()?,
            Kind::Packed { signed } => {
                let v = bits(record, self.offset_bits, self.size_bits)?;
                let negative = signed && v >> (self.size_bits - 1) & 1 == 1;
                vec![if negative && self.size_bits < 64 {
                    v | !0u64 << self.size_bits
                } else {
                    v
                }]
            }
            Kind::Common { default, at, len } => {
                // Pairs of (row id, value) for the rows that differ.
                let hit = (0..len / 8).find_map(|i| {
                    (common.u32(at + i * 8)? == id).then(|| common.u32(at + i * 8 + 4))?
                });
                vec![hit.unwrap_or(default) as u64]
            }
            Kind::Pallet { at, len } => {
                let i = index()?;
                (i < len / 4).then_some(())?;
                vec![pallet.u32(at + i * 4)? as u64]
            }
            Kind::PalletArray { at, len, count } => {
                let first = index()?.checked_mul(count)?;
                (first.checked_add(count)? <= len / 4).then_some(())?;
                (0..count)
                    .map(|j| pallet.u32(at + (first + j) * 4).map(u64::from))
                    .collect::<Option<_>>()?
            }
        })
    }
}

impl Table {
    /// Read `fields` (column numbers, from 0) of every row.
    pub fn parse(data: &[u8], fields: &[usize]) -> Result<Self, String> {
        Self::parse_inner(data, fields)
            .ok_or_else(|| "table is cut short or damaged".to_string())?
    }

    /// Outer `None`: ran off the end of the file somewhere.
    fn parse_inner(data: &[u8], wanted: &[usize]) -> Option<Result<Self, String>> {
        let file = Bytes(data);
        if file.slice(0, 4)? != b"WDC5" {
            return Some(Err("not a WDC5 database table".into()));
        }
        // After the magic: a version, then a 128-byte build name.
        let h = 136;
        let record_size = file.size(h + 8)?;
        let layout_hash = file.u32(h + 20)?;
        let flags = file.u16(h + 36)?;
        let id_column = file.u16(h + 38)? as usize;
        let field_count = file.size(h + 40)?;
        let storage_size = file.size(h + 52)?;
        let common_size = file.size(h + 56)?;
        let pallet_size = file.size(h + 60)?;
        let section_count = file.size(h + 64)?;

        if flags & FLAG_VARIABLE_RECORDS != 0 {
            return Some(Err(
                "tables with variable-size records are not supported".into()
            ));
        }
        if let Some(bad) = wanted.iter().find(|&&f| f >= field_count) {
            return Some(Err(format!(
                "column {bad} asked for, but the table has {field_count}"
            )));
        }
        if storage_size / FIELD_STORAGE_SIZE != field_count || id_column >= field_count.max(1) {
            return Some(Err("field tables disagree about the column count".into()));
        }

        let sections_at = HEADER_SIZE;
        let structure_at =
            sections_at.checked_add(section_count.checked_mul(SECTION_HEADER_SIZE)?)?;
        let storage_at = structure_at.checked_add(field_count.checked_mul(4)?)?;
        let pallet_at = storage_at.checked_add(storage_size)?;
        let common_at = pallet_at.checked_add(pallet_size)?;
        let pallet = Bytes(file.slice(pallet_at, pallet_size)?);
        let common = Bytes(file.slice(common_at, common_size)?);

        // Pallet and common fields each own a run of their side table, in
        // column order.
        let (mut pallet_used, mut common_used) = (0usize, 0usize);
        let mut columns = Vec::with_capacity(field_count);
        for i in 0..field_count {
            let s = storage_at + i * FIELD_STORAGE_SIZE;
            let side_len = file.size(s + 4)?;
            let (v1, v3) = (file.u32(s + 12)?, file.size(s + 20)?);
            let kind = match file.u32(s + 8)? {
                0 => {
                    // Stored as "32 minus the element's size in bits".
                    let stored =
                        i16::from_le_bytes(file.slice(structure_at + i * 4, 2)?.try_into().ok()?);
                    let element_bits = 32 - stored as i32;
                    if !(1..=64).contains(&element_bits) {
                        return Some(Err(format!("column {i} has an impossible element size")));
                    }
                    Kind::Plain {
                        element_bits: element_bits as usize,
                    }
                }
                1 => Kind::Packed { signed: false },
                5 => Kind::Packed { signed: true },
                2 => {
                    let at = common_used;
                    common_used = common_used.checked_add(side_len)?;
                    Kind::Common {
                        default: v1,
                        at,
                        len: side_len,
                    }
                }
                kind @ (3 | 4) => {
                    let at = pallet_used;
                    pallet_used = pallet_used.checked_add(side_len)?;
                    if kind == 3 {
                        Kind::Pallet { at, len: side_len }
                    } else {
                        Kind::PalletArray {
                            at,
                            len: side_len,
                            count: v3,
                        }
                    }
                }
                other => return Some(Err(format!("column {i} uses unknown storage {other}"))),
            };
            columns.push(Field {
                offset_bits: file.u16(s)? as usize,
                size_bits: file.u16(s + 2)? as usize,
                kind,
            });
        }
        if pallet_used > pallet_size || common_used > common_size {
            return Some(Err("side tables are smaller than the columns claim".into()));
        }

        let mut rows: Vec<Row> = Vec::new();
        for section in 0..section_count {
            let s = sections_at + section * SECTION_HEADER_SIZE;
            let key = file.u64(s)?;
            let at = file.size(s + 8)?;
            let count = file.size(s + 12)?;
            let strings = file.size(s + 16)?;
            let id_list_size = file.size(s + 24)?;
            let copy_count = file.size(s + 36)?;

            let records = file.slice(at, count.checked_mul(record_size)?)?;
            let ids_at = at.checked_add(records.len())?.checked_add(strings)?;
            let ids = Bytes(file.slice(ids_at, id_list_size)?);
            let copies_at = ids_at.checked_add(id_list_size)?;
            let copies = Bytes(file.slice(copies_at, copy_count.checked_mul(8)?)?);

            // Encrypted with a key we lack: handed to us as zeros.
            if key != 0 && records.iter().all(|&b| b == 0) {
                continue;
            }

            let first_row = rows.len();
            for (r, record) in records.chunks_exact(record_size.max(1)).enumerate() {
                let id = if id_list_size > 0 {
                    ids.u32(r * 4)?
                } else {
                    let own = columns.get(id_column)?.read(record, 0, &pallet, &common);
                    *own?.first()? as u32
                };
                let Some(fields) = wanted
                    .iter()
                    .map(|&f| columns[f].read(record, id, &pallet, &common))
                    .collect::<Option<Vec<_>>>()
                else {
                    return Some(Err(format!("row {id} points outside the table's data")));
                };
                rows.push(Row { id, fields });
            }
            for c in 0..copy_count {
                let (new, source) = (copies.u32(c * 8)?, copies.u32(c * 8 + 4)?);
                if let Some(row) = rows[first_row..].iter().find(|r| r.id == source) {
                    let fields = row.fields.clone();
                    rows.push(Row { id: new, fields });
                }
            }
        }

        Some(Ok(Self {
            layout_hash,
            field_count,
            rows,
        }))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes bit fields into a record, lowest bit first.
    #[derive(Default)]
    struct Bits(Vec<u8>);
    impl Bits {
        fn put(&mut self, offset: usize, size: usize, value: u64) {
            for i in 0..size {
                let bit = offset + i;
                if self.0.len() <= bit / 8 {
                    self.0.resize(bit / 8 + 1, 0);
                }
                if value >> i & 1 == 1 {
                    self.0[bit / 8] |= 1 << (bit % 8);
                }
            }
        }
    }

    const RECORD_SIZE: usize = 8;

    /// One record of the test table.  Columns:
    ///
    /// 0. plain u32
    /// 1. bit-packed, 10 bits
    /// 2. pallet index, 3 bits, into `[100, 200, 300]`
    /// 3. pallet array of 2, 2 bits, into `[[1, 2], [3, 4], [0, 0]]`
    /// 4. common, default 7
    /// 5. bit-packed signed, 4 bits
    pub fn record(plain: u32, packed: u64, pallet: u64, pallet_array: u64, signed: i64) -> Vec<u8> {
        let mut b = Bits::default();
        b.put(0, 32, plain as u64);
        b.put(32, 10, packed);
        b.put(42, 3, pallet);
        b.put(45, 2, pallet_array);
        b.put(47, 4, signed as u64 & 0xF);
        b.0.resize(RECORD_SIZE, 0);
        b.0
    }

    fn u32s(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// `(offset bits, size bits, side-table bytes, kind, v1, v2, v3)`
    fn storage(f: (u16, u16, u32, u32, u32, u32, u32)) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&f.0.to_le_bytes());
        b.extend_from_slice(&f.1.to_le_bytes());
        b.extend_from_slice(&u32s(&[f.2, f.3, f.4, f.5, f.6]));
        b
    }

    pub struct Section {
        pub key: u64,
        pub records: Vec<Vec<u8>>,
        pub ids: Vec<u32>,
        /// `(new id, id it copies)`
        pub copies: Vec<(u32, u32)>,
    }

    /// A whole WDC5 file with the test columns.
    pub fn file(flags: u16, sections: &[Section]) -> Vec<u8> {
        let pallet = u32s(&[100, 200, 300, 1, 2, 3, 4, 0, 0]);
        let common = u32s(&[20, 99]); // row 20 has 99 in column 4
        let fields = [
            storage((0, 32, 0, 0, 0, 0, 0)),
            storage((32, 10, 0, 1, 0, 0, 0)),
            storage((42, 3, 12, 3, 0, 0, 0)),
            storage((45, 2, 24, 4, 0, 0, 2)),
            storage((0, 0, 8, 2, 7, 0, 0)),
            storage((47, 4, 0, 5, 0, 0, 0)),
        ]
        .concat();
        let n_fields = 6u32;
        let total_records: u32 = sections.iter().map(|s| s.records.len() as u32).sum();

        let mut f = Vec::new();
        f.extend_from_slice(b"WDC5");
        f.extend_from_slice(&5u32.to_le_bytes());
        f.extend_from_slice(&[0u8; 128]);
        f.extend_from_slice(&u32s(&[
            total_records,
            n_fields,
            RECORD_SIZE as u32,
            0,           // string table size
            0x1111_1111, // table hash
            0xABCD_0123, // layout hash
            1,
            1000,
            0xFFFF_FFFF, // locale
        ]));
        f.extend_from_slice(&flags.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes()); // id column
        f.extend_from_slice(&u32s(&[
            n_fields,
            0, // bit-packed data offset
            0, // lookup column count
            fields.len() as u32,
            common.len() as u32,
            pallet.len() as u32,
            sections.len() as u32,
        ]));

        let headers_at = f.len();
        f.resize(headers_at + sections.len() * 40, 0);
        for _ in 0..n_fields {
            f.extend_from_slice(&0i16.to_le_bytes()); // 32-bit elements
            f.extend_from_slice(&0u16.to_le_bytes());
        }
        f.extend_from_slice(&fields);
        f.extend_from_slice(&pallet);
        f.extend_from_slice(&common);

        for (i, s) in sections.iter().enumerate() {
            let at = f.len() as u32;
            let mut h = s.key.to_le_bytes().to_vec();
            h.extend_from_slice(&u32s(&[
                at,
                s.records.len() as u32,
                0, // string table size
                0, // offset records end
                s.ids.len() as u32 * 4,
                0, // relationship data size
                0, // offset map id count
                s.copies.len() as u32,
            ]));
            f[headers_at + i * 40..headers_at + (i + 1) * 40].copy_from_slice(&h);
            for r in &s.records {
                f.extend_from_slice(r);
            }
            f.extend_from_slice(&u32s(&s.ids));
            for (new, old) in &s.copies {
                f.extend_from_slice(&u32s(&[*new, *old]));
            }
        }
        f
    }

    pub const HAS_ID_LIST: u16 = 4;

    fn two_rows() -> Vec<u8> {
        file(
            HAS_ID_LIST,
            &[Section {
                key: 0,
                records: vec![record(5000, 777, 1, 0, -3), record(6000, 1, 2, 1, 5)],
                ids: vec![10, 20],
                copies: vec![(30, 10)],
            }],
        )
    }

    fn row(t: &Table, id: u32) -> &Row {
        t.rows.iter().find(|r| r.id == id).expect("row")
    }

    #[test]
    fn reads_every_kind_of_field() {
        let t = Table::parse(&two_rows(), &[0, 1, 2, 3, 4, 5]).unwrap();
        assert_eq!(t.layout_hash, 0xABCD_0123);
        assert_eq!(t.field_count, 6);
        assert_eq!(
            row(&t, 10).fields,
            [
                vec![5000],
                vec![777],
                vec![200],
                vec![1, 2],
                vec![7],
                vec![-3i64 as u64]
            ]
        );
        assert_eq!(
            row(&t, 20).fields,
            [
                vec![6000],
                vec![1],
                vec![300],
                vec![3, 4],
                vec![99],
                vec![5]
            ]
        );
    }

    #[test]
    fn returns_only_the_fields_asked_for_in_that_order() {
        let t = Table::parse(&two_rows(), &[3, 1]).unwrap();
        assert_eq!(row(&t, 10).fields, [vec![1, 2], vec![777]]);
    }

    #[test]
    fn a_copied_row_has_its_source_rows_values_under_the_new_id() {
        let t = Table::parse(&two_rows(), &[1, 4]).unwrap();
        assert_eq!(row(&t, 30).fields, [vec![777], vec![7]]);
        assert_eq!(t.rows.len(), 3);
    }

    #[test]
    fn without_an_id_list_the_id_is_the_id_column() {
        let f = file(
            0,
            &[Section {
                key: 0,
                records: vec![record(42, 9, 0, 0, 0)],
                ids: vec![],
                copies: vec![],
            }],
        );
        let t = Table::parse(&f, &[1]).unwrap();
        assert_eq!(
            t.rows,
            [Row {
                id: 42,
                fields: vec![vec![9]]
            }]
        );
    }

    /// A section we could not decrypt arrives as zeros and is left out.
    #[test]
    fn an_encrypted_section_that_is_all_zeros_is_skipped() {
        let f = file(
            HAS_ID_LIST,
            &[
                Section {
                    key: 0,
                    records: vec![record(1, 2, 0, 0, 0)],
                    ids: vec![10],
                    copies: vec![],
                },
                Section {
                    key: 0xDEAD_BEEF,
                    records: vec![vec![0; RECORD_SIZE]; 2],
                    ids: vec![0, 0],
                    copies: vec![],
                },
            ],
        );
        let t = Table::parse(&f, &[1]).unwrap();
        assert_eq!(t.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [10]);
    }

    /// The same section with real data (we had the key) is read.
    #[test]
    fn an_encrypted_section_with_data_is_read() {
        let f = file(
            HAS_ID_LIST,
            &[Section {
                key: 0xDEAD_BEEF,
                records: vec![record(1, 2, 0, 0, 0)],
                ids: vec![10],
                copies: vec![],
            }],
        );
        assert_eq!(Table::parse(&f, &[1]).unwrap().rows.len(), 1);
    }

    #[test]
    fn asking_for_a_column_the_table_does_not_have_is_an_error() {
        let err = Table::parse(&two_rows(), &[6]).unwrap_err();
        assert!(err.contains('6'), "{err}");
    }

    #[test]
    fn a_pallet_index_past_the_pallet_is_an_error_not_a_panic() {
        let f = file(
            HAS_ID_LIST,
            &[Section {
                key: 0,
                records: vec![record(1, 2, 7, 3, 0)],
                ids: vec![10],
                copies: vec![],
            }],
        );
        assert!(Table::parse(&f, &[2]).is_err());
        assert!(Table::parse(&f, &[3]).is_err());
        assert!(
            Table::parse(&f, &[1]).is_ok(),
            "unread columns do not matter"
        );
    }

    #[test]
    fn truncated_and_foreign_files_are_errors() {
        let f = two_rows();
        assert!(Table::parse(b"", &[0]).is_err());
        assert!(Table::parse(b"WDC3aaaaaaaa", &[0]).is_err());
        for cut in [100, 200, 300, f.len() - 30, f.len() - 1] {
            assert!(Table::parse(&f[..cut], &[0, 2, 4]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn tables_with_variable_size_records_are_refused_by_name() {
        let err = Table::parse(&file(1, &[]), &[0]).unwrap_err();
        assert!(err.contains("variable"), "{err}");
    }
}
