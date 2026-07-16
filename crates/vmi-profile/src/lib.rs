use pdb::{FallibleIterator, SymbolData, PDB};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::Read,
    path::Path,
};
use vmi_types::{Result, VmiError};

pub const MAX_TEXT_PROFILE_SIZE: u64 = 64 * 1024 * 1024;
pub const MAX_PDB_SIZE: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_PROFILE_ENTRIES: usize = 1_000_000;

fn read_retry(reader: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn read_text_profile(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|error| {
        VmiError::Backend(format!("failed to open {}: {error}", path.display()))
    })?;
    let size = file
        .metadata()
        .map_err(|error| {
            VmiError::Backend(format!("failed to inspect {}: {error}", path.display()))
        })?
        .len();
    if size > MAX_TEXT_PROFILE_SIZE {
        return Err(VmiError::Backend(format!(
            "text profile exceeds {MAX_TEXT_PROFILE_SIZE} bytes"
        )));
    }
    let capacity = usize::try_from(size)
        .map_err(|_| VmiError::Backend("text profile is too large for this host".into()))?;
    let mut contents = Vec::new();
    contents.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate text profile buffer: {error}"))
    })?;
    let capture_limit = MAX_TEXT_PROFILE_SIZE + 1;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let captured = u64::try_from(contents.len())
            .map_err(|_| VmiError::Backend("text profile size does not fit u64".into()))?;
        let chunk_capacity = u64::try_from(chunk.len())
            .map_err(|_| VmiError::Backend("text profile chunk size does not fit u64".into()))?;
        let remaining = capture_limit.saturating_sub(captured);
        let requested = usize::try_from(remaining.min(chunk_capacity))
            .map_err(|_| VmiError::Backend("text profile chunk is too large".into()))?;
        if requested == 0 {
            break;
        }
        let requested_chunk = chunk.get_mut(..requested).ok_or_else(|| {
            VmiError::Backend("text profile read boundary invariant failed".into())
        })?;
        let count = read_retry(&mut file, requested_chunk).map_err(|error| {
            VmiError::Backend(format!("failed to read {}: {error}", path.display()))
        })?;
        if count == 0 {
            break;
        }
        contents.try_reserve(count).map_err(|error| {
            VmiError::Backend(format!("failed to grow text profile buffer: {error}"))
        })?;
        let captured = chunk.get(..count).ok_or_else(|| {
            VmiError::Backend("text profile result length exceeds read buffer".into())
        })?;
        contents.extend_from_slice(captured);
        if u64::try_from(contents.len()).map_or(true, |size| size > MAX_TEXT_PROFILE_SIZE) {
            return Err(VmiError::Backend(format!(
                "text profile exceeds {MAX_TEXT_PROFILE_SIZE} bytes"
            )));
        }
    }
    String::from_utf8(contents).map_err(|error| {
        VmiError::Backend(format!(
            "text profile {} is not UTF-8: {error}",
            path.display()
        ))
    })
}

fn open_pdb_file(path: &Path) -> Result<File> {
    let file = File::open(path).map_err(|error| {
        VmiError::Backend(format!("failed to open PDB {}: {error}", path.display()))
    })?;
    let size = file
        .metadata()
        .map_err(|error| {
            VmiError::Backend(format!("failed to inspect PDB {}: {error}", path.display()))
        })?
        .len();
    if size > MAX_PDB_SIZE {
        return Err(VmiError::Backend(format!(
            "PDB exceeds {MAX_PDB_SIZE} bytes"
        )));
    }
    Ok(file)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub address: u64,
    pub kind: Option<char>,
}

#[derive(Clone, Debug, Default)]
pub struct SymbolTable {
    by_name: HashMap<String, Symbol>,
    by_address: BTreeMap<u64, Vec<String>>,
}

impl SymbolTable {
    pub fn from_pdb_file(path: impl AsRef<Path>, image_base: u64) -> Result<Self> {
        let path = path.as_ref();
        let file = open_pdb_file(path)?;
        let mut pdb = PDB::open(file).map_err(pdb_error)?;
        let symbols = pdb.global_symbols().map_err(pdb_error)?;
        let address_map = pdb.address_map().map_err(pdb_error)?;
        let mut output = Self::default();
        let mut iterator = symbols.iter();
        while let Some(symbol) = iterator.next().map_err(pdb_error)? {
            let Ok(SymbolData::Public(public)) = symbol.parse() else {
                continue;
            };
            let Some(rva) = public.offset.to_rva(&address_map) else {
                continue;
            };
            let decoded_name = public.name.to_string();
            let name = try_clone_profile_string(decoded_name.as_ref(), "PDB symbol name")?;
            if name.is_empty() {
                continue;
            }
            if output.len() == MAX_PROFILE_ENTRIES {
                return Err(VmiError::Backend(format!(
                    "PDB exceeds {MAX_PROFILE_ENTRIES} public symbols"
                )));
            }
            let address = image_base.checked_add(u64::from(rva.0)).ok_or_else(|| {
                VmiError::Backend(format!("PDB symbol {name} address overflows image base"))
            })?;
            if let Some(existing) = output.symbol(&name) {
                if existing.address == address {
                    continue;
                }
                return Err(VmiError::Backend(format!(
                    "PDB contains conflicting public symbol {name}"
                )));
            }
            output.insert(Symbol {
                name,
                address,
                kind: Some(if public.function { 'T' } else { 'D' }),
            })?;
        }
        if output.is_empty() {
            return Err(VmiError::Backend(
                "PDB contains no addressable public symbols".into(),
            ));
        }
        Ok(output)
    }

    pub fn from_system_map_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = read_text_profile(path)?;
        Self::from_system_map(&contents)
    }

    pub fn from_system_map(contents: &str) -> Result<Self> {
        let mut table = Self::default();
        for (index, line) in contents.lines().enumerate() {
            let Some(line_number) = index.checked_add(1) else {
                return Err(VmiError::Backend("System.map line number overflow".into()));
            };
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if table.len() == MAX_PROFILE_ENTRIES {
                return Err(VmiError::Backend(format!(
                    "System.map exceeds {MAX_PROFILE_ENTRIES} symbols"
                )));
            }
            let mut fields = line.split_whitespace();
            let address_text = fields.next().ok_or_else(|| malformed(line_number))?;
            let kind_text = fields.next().ok_or_else(|| malformed(line_number))?;
            let name = fields.next().ok_or_else(|| malformed(line_number))?;
            if fields.next().is_some() || kind_text.chars().count() != 1 {
                return Err(malformed(line_number));
            }
            let address = u64::from_str_radix(address_text.trim_start_matches("0x"), 16).map_err(
                |error| {
                    VmiError::Backend(format!(
                        "invalid System.map address on line {line_number}: {error}"
                    ))
                },
            )?;
            let symbol = Symbol {
                name: try_clone_profile_string(name, "System.map symbol name")?,
                address,
                kind: kind_text.chars().next(),
            };
            table.insert(symbol).map_err(|_| {
                VmiError::Backend(format!(
                    "duplicate System.map symbol {name} on line {line_number}"
                ))
            })?;
        }
        if table.by_name.is_empty() {
            return Err(VmiError::Backend("System.map contains no symbols".into()));
        }
        Ok(table)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.by_name.get(name)
    }
    pub fn symbols_at(&self, address: u64) -> impl Iterator<Item = &Symbol> {
        self.by_address
            .get(&address)
            .into_iter()
            .flatten()
            .filter_map(|name| self.by_name.get(name))
    }
    pub fn nearest_symbol(&self, address: u64) -> Option<(&Symbol, u64)> {
        let (base, names) = self.by_address.range(..=address).next_back()?;
        let symbol = self.by_name.get(names.first()?)?;
        Some((symbol, address.checked_sub(*base)?))
    }

    fn insert(&mut self, symbol: Symbol) -> Result<()> {
        if self.by_name.contains_key(&symbol.name) {
            return Err(VmiError::Backend(format!(
                "duplicate symbol {}",
                symbol.name
            )));
        }
        self.by_name.try_reserve(1).map_err(|error| {
            VmiError::Backend(format!("failed to grow profile symbol table: {error}"))
        })?;
        let name_key = try_clone_profile_string(&symbol.name, "symbol name-index key")?;
        let alias_name = try_clone_profile_string(&symbol.name, "symbol address-index key")?;
        if let Some(names) = self.by_address.get_mut(&symbol.address) {
            names.try_reserve(1).map_err(|error| {
                VmiError::Backend(format!("failed to grow profile address aliases: {error}"))
            })?;
            names.push(alias_name);
        } else {
            let mut names = Vec::new();
            names.try_reserve_exact(1).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to allocate profile address aliases: {error}"
                ))
            })?;
            names.push(alias_name);
            self.by_address.insert(symbol.address, names);
        }
        self.by_name.insert(name_key, symbol);
        Ok(())
    }
}

fn try_clone_profile_string(value: &str, purpose: &str) -> Result<String> {
    let mut cloned = String::new();
    cloned.try_reserve_exact(value.len()).map_err(|error| {
        VmiError::Backend(format!("failed to allocate profile {purpose}: {error}"))
    })?;
    cloned.push_str(value);
    Ok(cloned)
}

fn pdb_error(error: pdb::Error) -> VmiError {
    VmiError::Backend(format!("PDB parsing failed: {error}"))
}

#[derive(Clone, Debug)]
pub struct Profile {
    symbols: SymbolTable,
    offsets: HashMap<String, u64>,
}

impl Profile {
    pub fn from_pdb_file(path: impl AsRef<Path>, image_base: u64) -> Result<Self> {
        let path = path.as_ref();
        let symbols = SymbolTable::from_pdb_file(path, image_base)?;
        let file = open_pdb_file(path)?;
        let mut pdb = PDB::open(file).map_err(pdb_error)?;
        let type_information = pdb.type_information().map_err(pdb_error)?;
        let mut finder = type_information.finder();
        let mut iterator = type_information.iter();
        while iterator.next().map_err(pdb_error)?.is_some() {
            finder.update(&iterator);
        }

        let mut offsets = HashMap::new();
        let mut iterator = type_information.iter();
        while let Some(item) = iterator.next().map_err(pdb_error)? {
            let Ok(pdb::TypeData::Class(class)) = item.parse() else {
                continue;
            };
            if class.properties.forward_reference() {
                continue;
            }
            let Some(mut field_index) = class.fields else {
                continue;
            };
            let decoded_class_name = class.name.to_string();
            let class_name =
                try_clone_profile_string(decoded_class_name.as_ref(), "PDB class name")?;
            if is_synthetic_pdb_type(&class_name) {
                continue;
            }
            let mut visited = std::collections::HashSet::new();
            loop {
                visited.try_reserve(1).map_err(|error| {
                    VmiError::Backend(format!(
                        "failed to grow PDB field-list cycle detector: {error}"
                    ))
                })?;
                if !visited.insert(field_index.0) {
                    return Err(VmiError::Backend(format!(
                        "PDB field list for {class_name} contains a cycle"
                    )));
                }
                let field_item = finder.find(field_index).map_err(pdb_error)?;
                let pdb::TypeData::FieldList(fields) = field_item.parse().map_err(pdb_error)?
                else {
                    return Err(VmiError::Backend(format!(
                        "PDB type {class_name} references a non-field-list record"
                    )));
                };
                for field in fields.fields {
                    if let pdb::TypeData::Member(member) = field {
                        let member_name = member.name.to_string();
                        let key = try_join_field_name(&class_name, member_name.as_ref())?;
                        insert_profile_offset(&mut offsets, key, member.offset, "PDB")?;
                    }
                }
                let Some(continuation) = fields.continuation else {
                    break;
                };
                field_index = continuation;
            }
        }
        Ok(Self { symbols, offsets })
    }

    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = read_text_profile(path)?;
        Self::from_json(&contents)
    }

    pub fn from_json(contents: &str) -> Result<Self> {
        let root: Value = serde_json::from_str(contents)
            .map_err(|error| VmiError::Backend(format!("invalid profile JSON: {error}")))?;
        let root = root
            .as_object()
            .ok_or_else(|| VmiError::Backend("profile root must be an object".into()))?;
        let symbol_values = root
            .get("symbols")
            .and_then(Value::as_object)
            .ok_or_else(|| VmiError::Backend("profile symbols must be an object".into()))?;
        let offset_values = root
            .get("offsets")
            .and_then(Value::as_object)
            .ok_or_else(|| VmiError::Backend("profile offsets must be an object".into()))?;
        if symbol_values.len() > MAX_PROFILE_ENTRIES || offset_values.len() > MAX_PROFILE_ENTRIES {
            return Err(VmiError::Backend(format!(
                "profile symbols or offsets exceed {MAX_PROFILE_ENTRIES} entries"
            )));
        }
        let mut symbols = SymbolTable::default();
        for (name, value) in symbol_values {
            symbols.insert(Symbol {
                name: try_clone_profile_string(name, "JSON symbol name")?,
                address: profile_number(value, name)?,
                kind: None,
            })?;
        }
        if symbols.is_empty() {
            return Err(VmiError::Backend("profile contains no symbols".into()));
        }
        let mut offsets = HashMap::new();
        offsets.try_reserve(offset_values.len()).map_err(|error| {
            VmiError::Backend(format!("failed to allocate profile offset table: {error}"))
        })?;
        for (name, value) in offset_values {
            offsets.insert(
                try_clone_profile_string(name, "JSON offset name")?,
                profile_number(value, name)?,
            );
        }
        Ok(Self { symbols, offsets })
    }

    pub fn symbols(&self) -> &SymbolTable {
        &self.symbols
    }
    pub fn offset(&self, name: &str) -> Option<u64> {
        self.offsets.get(name).copied()
    }
    pub fn require_offset(&self, name: &str) -> Result<u64> {
        self.offset(name)
            .ok_or_else(|| VmiError::Backend(format!("profile offset {name} not found")))
    }

    pub fn offsets_len(&self) -> usize {
        self.offsets.len()
    }
}

fn try_join_field_name(class_name: &str, member_name: &str) -> Result<String> {
    let capacity = class_name
        .len()
        .checked_add(1)
        .and_then(|length| length.checked_add(member_name.len()))
        .ok_or_else(|| VmiError::Backend("PDB field name length overflow".into()))?;
    let mut key = String::new();
    key.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate PDB field name: {error}"))
    })?;
    key.push_str(class_name);
    key.push('.');
    key.push_str(member_name);
    Ok(key)
}

fn insert_profile_offset(
    offsets: &mut HashMap<String, u64>,
    key: String,
    offset: u64,
    source: &str,
) -> Result<()> {
    if let Some(previous) = offsets.get(&key) {
        if *previous == offset {
            return Ok(());
        }
        return Err(VmiError::Backend(format!(
            "{source} contains conflicting field offset {key}"
        )));
    }
    if offsets.len() == MAX_PROFILE_ENTRIES {
        return Err(VmiError::Backend(format!(
            "{source} exceeds {MAX_PROFILE_ENTRIES} field offsets"
        )));
    }
    offsets.try_reserve(1).map_err(|error| {
        VmiError::Backend(format!(
            "failed to grow {source} field-offset table: {error}"
        ))
    })?;
    offsets.insert(key, offset);
    Ok(())
}

fn is_synthetic_pdb_type(name: &str) -> bool {
    name.contains("closure_env$")
}

fn profile_number(value: &Value, field: &str) -> Result<u64> {
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    let text = value.as_str().ok_or_else(|| {
        VmiError::Backend(format!(
            "profile value {field} must be an unsigned number or string"
        ))
    })?;
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
            .map_err(|error| VmiError::Backend(format!("invalid profile value {field}: {error}")))
    } else {
        text.parse()
            .map_err(|error| VmiError::Backend(format!("invalid profile value {field}: {error}")))
    }
}

fn malformed(line: usize) -> VmiError {
    VmiError::Backend(format!("malformed System.map line {line}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct InterruptOnce<R> {
        inner: R,
        interrupted: bool,
    }

    impl<R: Read> Read for InterruptOnce<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            self.inner.read(buffer)
        }
    }

    #[test]
    fn text_reader_retries_interrupted_reads() {
        let mut reader = InterruptOnce {
            inner: &b"profile"[..],
            interrupted: false,
        };
        let mut output = [0; 7];
        assert_eq!(read_retry(&mut reader, &mut output).unwrap(), 7);
        assert_eq!(&output, b"profile");
    }

    #[test]
    fn parses_and_looks_up_linux_symbols() {
        let table = SymbolTable::from_system_map(
            "ffffffff81000000 T _text\nffffffff81000100 t startup_64\nffffffff81000100 T alias\n",
        )
        .unwrap();
        assert_eq!(table.len(), 3);
        assert_eq!(
            table.symbol("_text").unwrap().address,
            0xffff_ffff_8100_0000
        );
        assert_eq!(table.symbols_at(0xffff_ffff_8100_0100).count(), 2);
        let (symbol, offset) = table.nearest_symbol(0xffff_ffff_8100_0123).unwrap();
        assert_eq!(symbol.address, 0xffff_ffff_8100_0100);
        assert_eq!(offset, 0x23);
    }

    #[test]
    fn rejects_empty_malformed_and_duplicate_profiles() {
        assert!(SymbolTable::from_system_map("").is_err());
        assert!(SymbolTable::from_system_map("not-an-address T symbol").is_err());
        assert!(SymbolTable::from_system_map("1 T same\n2 T same\n").is_err());
    }

    #[test]
    fn duplicate_insertion_does_not_mutate_address_index() {
        let mut table = SymbolTable::default();
        table
            .insert(Symbol {
                name: "stable".into(),
                address: 1,
                kind: Some('T'),
            })
            .unwrap();
        assert!(table
            .insert(Symbol {
                name: "stable".into(),
                address: 2,
                kind: Some('D'),
            })
            .is_err());
        assert_eq!(table.len(), 1);
        assert_eq!(table.symbols_at(1).count(), 1);
        assert_eq!(table.symbols_at(2).count(), 0);
    }

    #[test]
    fn conflicting_field_offset_does_not_replace_original() {
        let mut offsets = HashMap::new();
        let key = try_join_field_name("_TYPE", "field").unwrap();
        insert_profile_offset(&mut offsets, key.clone(), 8, "test").unwrap();
        insert_profile_offset(&mut offsets, key.clone(), 8, "test").unwrap();
        assert!(insert_profile_offset(&mut offsets, key.clone(), 16, "test").is_err());
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets.get(&key), Some(&8));
    }

    #[test]
    fn excludes_unstable_compiler_generated_pdb_closure_types() {
        assert!(is_synthetic_pdb_type("crate::function::closure_env$0"));
        assert!(!is_synthetic_pdb_type("kernel::_EPROCESS"));
    }

    #[test]
    fn parses_normalized_json_symbols_and_offsets() {
        let profile = Profile::from_json(
            r#"{
                "symbols": {
                    "PsActiveProcessHead": "0xfffff80000100000",
                    "init_task": 4096
                },
                "offsets": {
                    "_EPROCESS.ActiveProcessLinks": "0x448",
                    "task_struct.pid": 64
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            profile
                .symbols()
                .symbol("PsActiveProcessHead")
                .unwrap()
                .address,
            0xffff_f800_0010_0000
        );
        assert_eq!(
            profile
                .require_offset("_EPROCESS.ActiveProcessLinks")
                .unwrap(),
            0x448
        );
        assert_eq!(profile.require_offset("task_struct.pid").unwrap(), 64);
        assert!(profile.require_offset("missing").is_err());
    }

    #[test]
    fn rejects_malformed_normalized_profiles() {
        assert!(Profile::from_json("not json").is_err());
        assert!(Profile::from_json(r#"{"symbols": {}, "offsets": {}}"#).is_err());
        assert!(Profile::from_json(r#"{"symbols": {"x": -1}, "offsets": {}}"#).is_err());
        assert!(Profile::from_json(r#"{"symbols": {"x": "bad"}, "offsets": {}}"#).is_err());
    }

    #[test]
    fn rejects_missing_and_malformed_pdb_files() {
        assert!(SymbolTable::from_pdb_file("definitely-missing.pdb", 0).is_err());
        let path = std::env::temp_dir().join(format!(
            "vmi-profile-{}.pdb",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"not a pdb").unwrap();
        assert!(SymbolTable::from_pdb_file(&path, 0xffff_f800_0000_0000).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_non_utf8_text_profile_files() {
        let path =
            std::env::temp_dir().join(format!("vmi-profile-utf8-{}.json", std::process::id()));
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(Profile::from_json_file(&path)
            .unwrap_err()
            .to_string()
            .contains("not UTF-8"));
        assert!(SymbolTable::from_system_map_file(&path)
            .unwrap_err()
            .to_string()
            .contains("not UTF-8"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_oversized_text_profile_files_before_parsing() {
        let path =
            std::env::temp_dir().join(format!("vmi-profile-large-{}.json", std::process::id()));
        let file = File::create(&path).unwrap();
        file.set_len(MAX_TEXT_PROFILE_SIZE + 1).unwrap();
        drop(file);
        assert!(Profile::from_json_file(&path).is_err());
        assert!(SymbolTable::from_system_map_file(&path).is_err());
        fs::remove_file(path).unwrap();

        let pdb_path =
            std::env::temp_dir().join(format!("vmi-profile-large-{}.pdb", std::process::id()));
        let file = File::create(&pdb_path).unwrap();
        file.set_len(MAX_PDB_SIZE + 1).unwrap();
        drop(file);
        assert!(SymbolTable::from_pdb_file(&pdb_path, 0).is_err());
        assert!(Profile::from_pdb_file(&pdb_path, 0).is_err());
        fs::remove_file(pdb_path).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_public_symbols_from_rust_msvc_pdb() {
        let pdb = std::env::current_exe().unwrap().with_extension("pdb");
        let symbols = SymbolTable::from_pdb_file(&pdb, 0x1_4000_0000).unwrap();
        assert!(!symbols.is_empty());
        assert!(symbols
            .by_name
            .values()
            .all(|symbol| symbol.address >= 0x1_4000_0000));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_type_field_offsets_from_rust_msvc_pdb() {
        let pdb = std::env::current_exe().unwrap().with_extension("pdb");
        let profile = Profile::from_pdb_file(&pdb, 0x1_4000_0000).unwrap();
        assert!(!profile.symbols().is_empty());
        assert!(profile.offsets_len() > 0);
    }
}
