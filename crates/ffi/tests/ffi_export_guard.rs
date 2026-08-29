use std::collections::{BTreeMap, BTreeSet};

struct ExportedFn {
    line_number: usize,
    name: String,
    signature: String,
    body: String,
}

#[derive(Debug, Eq, PartialEq)]
struct AbiSignature {
    return_type: String,
    args: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct FieldShape {
    name: String,
    ty: String,
}

#[derive(Debug, Eq, PartialEq)]
struct EnumShape {
    variants: Vec<(String, i64)>,
}

fn brace_delta(line: &str) -> isize {
    line.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

fn exported_fn_name(signature: &str) -> &str {
    signature
        .split_once("fn ")
        .and_then(|(_, rest)| {
            rest.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .next()
        })
        .unwrap_or("<unknown>")
}

fn exported_fns(source: &str) -> Vec<ExportedFn> {
    let mut lines = source.lines().enumerate().peekable();
    let mut exports = Vec::new();

    while let Some((line_index, line)) = lines.next() {
        if line != "#[unsafe(no_mangle)]" {
            continue;
        }

        let mut signature = String::new();
        for (_, next_line) in lines.by_ref() {
            if next_line.starts_with("#[") {
                continue;
            }
            signature.push_str(next_line);
            signature.push('\n');
            if next_line.contains('{') {
                break;
            }
        }

        let mut body = signature.clone();
        let mut depth = brace_delta(&signature);
        while depth > 0 {
            let Some((_, next_line)) = lines.next() else {
                break;
            };
            body.push_str(next_line);
            body.push('\n');
            depth += brace_delta(next_line);
        }

        exports.push(ExportedFn {
            line_number: line_index + 1,
            name: exported_fn_name(&signature).to_string(),
            signature,
            body,
        });
    }

    exports
}

fn header_function_names(header: &str) -> BTreeSet<String> {
    let bytes = header.as_bytes();
    let mut names = BTreeSet::new();
    let mut cursor = 0usize;

    while let Some(offset) = header[cursor..].find("termy_") {
        let start = cursor + offset;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }

        let mut next = end;
        while next < bytes.len() && bytes[next].is_ascii_whitespace() {
            next += 1;
        }
        if bytes.get(next) == Some(&b'(') {
            names.insert(header[start..end].to_string());
        }

        cursor = end;
    }

    names
}

fn canonical_rust_type(raw_type: &str) -> String {
    let raw_type = raw_type.trim().trim_end_matches(',').trim();
    if let Some(inner) = raw_type
        .strip_prefix("Option<")
        .and_then(|value| value.strip_suffix('>'))
    {
        return canonical_rust_type(inner);
    }
    if let Some(rest) = raw_type.strip_prefix("*const ") {
        return format!("*const {}", canonical_rust_type(rest));
    }
    if let Some(rest) = raw_type.strip_prefix("*mut ") {
        return format!("*mut {}", canonical_rust_type(rest));
    }

    match raw_type {
        "bool" => "bool".to_string(),
        "i32" => "i32".to_string(),
        "u8" => "u8".to_string(),
        "u16" => "u16".to_string(),
        "u32" => "u32".to_string(),
        "u64" => "u64".to_string(),
        "usize" => "usize".to_string(),
        "termy_tmux_control_core::session::ControlSession" => "TermyFfiTmuxControl".to_string(),
        other => other.to_string(),
    }
}

fn rust_export_signature(export: &ExportedFn) -> AbiSignature {
    let signature = export.signature.replace('\n', " ");
    let open_paren = signature
        .find('(')
        .unwrap_or_else(|| panic!("{} has no argument list", export.name));
    let close_paren = signature
        .rfind(')')
        .unwrap_or_else(|| panic!("{} has no argument list terminator", export.name));
    let args = signature[open_paren + 1..close_paren]
        .split(',')
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .map(|arg| {
            let (_, ty) = arg
                .split_once(": ")
                .unwrap_or_else(|| panic!("{} has an unparsable arg: {arg}", export.name));
            canonical_rust_type(ty)
        })
        .collect();

    let return_tail = signature[close_paren + 1..]
        .split_once('{')
        .map_or("", |(tail, _)| tail)
        .trim();
    let return_type = return_tail
        .strip_prefix("->")
        .map_or_else(|| "void".to_string(), canonical_rust_type);

    AbiSignature { return_type, args }
}

fn canonical_c_base_type(raw_type: &str) -> &str {
    match raw_type {
        "bool" => "bool",
        "float" => "f32",
        "int32_t" => "i32",
        "uint8_t" => "u8",
        "uint16_t" => "u16",
        "uint32_t" => "u32",
        "uint64_t" => "u64",
        "size_t" => "usize",
        other => other,
    }
}

fn trim_c_param_name(param: &str) -> &str {
    let trimmed = param.trim();
    let Some(name_start) = trimmed
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (!ch.is_ascii_alphanumeric() && ch != '_').then_some(index + 1))
    else {
        return trimmed;
    };
    trimmed[..name_start].trim()
}

fn canonical_c_type(raw_type: &str) -> String {
    let pointer_count = raw_type.chars().filter(|ch| *ch == '*').count();
    let no_pointer_type = raw_type.replace('*', " ");
    let mut parts = no_pointer_type.split_whitespace().collect::<Vec<_>>();
    let is_const = parts.first() == Some(&"const");
    parts.retain(|part| *part != "const");

    let base_type = canonical_c_base_type(&parts.join(" ")).to_string();
    (0..pointer_count).fold(base_type, |ty, depth| {
        let pointer_kind = if depth == 0 && is_const {
            "const"
        } else {
            "mut"
        };
        format!("*{pointer_kind} {ty}")
    })
}

fn header_function_signatures(header: &str) -> BTreeMap<String, AbiSignature> {
    let mut signatures = BTreeMap::new();
    let mut declaration = String::new();

    for line in header.lines().map(str::trim) {
        if declaration.is_empty() && !line.contains("termy_") {
            continue;
        }

        declaration.push_str(line);
        declaration.push(' ');
        if line.ends_with(';') {
            if let Some((name, signature)) = parse_header_declaration(&declaration) {
                signatures.insert(name, signature);
            }
            declaration.clear();
        }
    }

    signatures
}

fn parse_header_declaration(declaration: &str) -> Option<(String, AbiSignature)> {
    let declaration = declaration.trim().trim_end_matches(';').trim();
    let open_paren = declaration.find('(')?;
    let close_paren = declaration.rfind(')')?;
    let before_args = declaration[..open_paren].trim();
    let name_start = before_args.rfind(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')? + 1;
    let name = before_args[name_start..].to_string();
    let return_type = canonical_c_type(before_args[..name_start].trim());
    let args_text = declaration[open_paren + 1..close_paren].trim();
    let args = if args_text.is_empty() || args_text == "void" {
        Vec::new()
    } else {
        args_text
            .split(',')
            .map(trim_c_param_name)
            .map(canonical_c_type)
            .collect()
    };

    Some((name, AbiSignature { return_type, args }))
}

fn repr_c_blocks(source: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut lines = source.lines().peekable();

    while let Some(line) = lines.next() {
        if line.trim() != "#[repr(C)]" {
            continue;
        }

        let mut block = String::new();
        let mut depth = 0isize;
        for next_line in lines.by_ref() {
            let trimmed = next_line.trim();
            if block.is_empty() && trimmed.starts_with("#[") {
                continue;
            }
            block.push_str(next_line);
            block.push('\n');
            depth += brace_delta(next_line);
            if !block.is_empty() && depth == 0 && next_line.contains('}') {
                break;
            }
        }
        if block.contains("pub struct TermyFfi") || block.contains("pub enum TermyFfi") {
            blocks.push(block);
        }
    }

    blocks
}

fn rust_item_name(block: &str, marker: &str) -> Option<String> {
    let (_, rest) = block.split_once(marker)?;
    Some(
        rest.trim_start()
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .next()
            .unwrap_or("<unknown>")
            .to_string(),
    )
}

fn rust_repr_c_struct_shapes(source: &str) -> BTreeMap<String, Vec<FieldShape>> {
    repr_c_blocks(source)
        .into_iter()
        .filter_map(|block| {
            let name = rust_item_name(&block, "pub struct ")?;
            let fields = block
                .lines()
                .map(str::trim)
                .filter_map(|line| {
                    let field = line.strip_prefix("pub ")?;
                    let (name, ty) = field.split_once(": ")?;
                    Some(FieldShape {
                        name: name.to_string(),
                        ty: canonical_rust_type(ty),
                    })
                })
                .collect();
            Some((name, fields))
        })
        .collect()
}

fn rust_repr_c_enum_shapes(source: &str) -> BTreeMap<String, EnumShape> {
    repr_c_blocks(source)
        .into_iter()
        .filter_map(|block| {
            let name = rust_item_name(&block, "pub enum ")?;
            let variants = block
                .lines()
                .map(str::trim)
                .filter_map(|line| {
                    let (variant, value) = line.split_once(" = ")?;
                    Some((
                        variant.trim().to_string(),
                        value.trim_end_matches(',').parse().ok()?,
                    ))
                })
                .collect();
            Some((name, EnumShape { variants }))
        })
        .collect()
}

fn parse_c_field(line: &str) -> Option<FieldShape> {
    let line = line.trim().trim_end_matches(';').trim();
    if line.is_empty() {
        return None;
    }
    let name_start = line
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (!ch.is_ascii_alphanumeric() && ch != '_').then_some(index + 1))?;
    Some(FieldShape {
        name: line[name_start..].to_string(),
        ty: canonical_c_type(line[..name_start].trim()),
    })
}

fn header_struct_shapes(header: &str) -> BTreeMap<String, Vec<FieldShape>> {
    let mut structs = BTreeMap::new();
    let mut lines = header.lines().peekable();

    while let Some(line) = lines.next() {
        if line.trim() != "typedef struct {" {
            continue;
        }

        let mut fields = Vec::new();
        for next_line in lines.by_ref() {
            let trimmed = next_line.trim();
            if let Some(rest) = trimmed.strip_prefix("}") {
                let name = rest.trim().trim_end_matches(';').to_string();
                structs.insert(name, fields);
                break;
            }
            if let Some(field) = parse_c_field(trimmed) {
                fields.push(field);
            }
        }
    }

    structs
}

fn upper_snake_to_pascal(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut converted = String::new();
            converted.push(first.to_ascii_uppercase());
            converted.extend(chars.map(|ch| ch.to_ascii_lowercase()));
            converted
        })
        .collect()
}

fn header_enum_variant_name(name: &str) -> String {
    name.strip_prefix("TERMY_FFI_")
        .map_or_else(|| name.to_string(), upper_snake_to_pascal)
}

fn header_enum_shapes(header: &str) -> BTreeMap<String, EnumShape> {
    let mut enums = BTreeMap::new();
    let mut lines = header.lines().peekable();

    while let Some(line) = lines.next() {
        if line.trim() != "typedef enum {" {
            continue;
        }

        let mut variants = Vec::new();
        for next_line in lines.by_ref() {
            let trimmed = next_line.trim();
            if let Some(rest) = trimmed.strip_prefix("}") {
                let name = rest.trim().trim_end_matches(';').to_string();
                enums.insert(name, EnumShape { variants });
                break;
            }
            if trimmed.starts_with("/*") {
                continue;
            }
            if let Some((variant, value)) = trimmed.split_once(" = ") {
                let value = value.trim_end_matches(',').parse().unwrap_or_else(|_| {
                    panic!("unparsable enum value in termy.h: {trimmed}");
                });
                variants.push((header_enum_variant_name(variant.trim()), value));
            }
        }
    }

    enums
}

#[test]
fn status_returning_exports_use_panic_guard() {
    let exports = exported_fns(include_str!("../src/lib.rs"));
    let mut missing = Vec::new();
    let mut checked = 0usize;

    for export in exports {
        if export.signature.contains("TermyFfiStatus") {
            checked += 1;
            if !export.body.contains("ffi_status_guard") {
                missing.push(format!("{}:{}", export.line_number, export.name));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "status-returning FFI exports must use ffi_status_guard: {missing:?}"
    );
    assert!(checked > 0, "expected at least one status-returning export");
}

#[test]
fn extern_c_exports_use_panic_guard() {
    let exports = exported_fns(include_str!("../src/lib.rs"));
    let mut missing = Vec::new();

    for export in &exports {
        if !export.body.contains("ffi_status_guard") && !export.body.contains("ffi_guard") {
            missing.push(format!("{}:{}", export.line_number, export.name));
        }
    }

    assert!(
        missing.is_empty(),
        "FFI exports must catch panics before they cross the C ABI: {missing:?}"
    );
    assert!(!exports.is_empty(), "expected exported Rust functions");
}

#[test]
fn c_header_declares_all_rust_exports() {
    let rust_exports = exported_fns(include_str!("../src/lib.rs"))
        .into_iter()
        .map(|export| export.name)
        .collect::<BTreeSet<_>>();
    let header_exports = header_function_names(include_str!("../include/termy.h"));

    let missing_from_header = rust_exports
        .difference(&header_exports)
        .cloned()
        .collect::<Vec<_>>();
    let missing_from_rust = header_exports
        .difference(&rust_exports)
        .cloned()
        .collect::<Vec<_>>();

    assert!(
        missing_from_header.is_empty(),
        "Rust exports missing from termy.h: {missing_from_header:?}"
    );
    assert!(
        missing_from_rust.is_empty(),
        "termy.h declarations missing Rust exports: {missing_from_rust:?}"
    );
    assert!(!rust_exports.is_empty(), "expected exported Rust functions");
}

#[test]
fn c_header_signatures_match_rust_exports() {
    let rust_signatures = exported_fns(include_str!("../src/lib.rs"))
        .into_iter()
        .map(|export| (export.name.clone(), rust_export_signature(&export)))
        .collect::<BTreeMap<_, _>>();
    let header_signatures = header_function_signatures(include_str!("../include/termy.h"));

    let mismatches = rust_signatures
        .iter()
        .filter_map(|(name, rust_signature)| {
            header_signatures.get(name).and_then(|header_signature| {
                (header_signature != rust_signature).then(|| {
                    format!("{name}: rust={rust_signature:?}, header={header_signature:?}")
                })
            })
        })
        .collect::<Vec<_>>();

    assert!(
        mismatches.is_empty(),
        "termy.h signatures drifted from Rust exports: {mismatches:#?}"
    );
    assert!(
        !rust_signatures.is_empty(),
        "expected exported Rust functions"
    );
}

#[test]
fn c_header_struct_layouts_match_rust_repr_c_types() {
    let rust_structs = rust_repr_c_struct_shapes(include_str!("../src/lib.rs"));
    let header_structs = header_struct_shapes(include_str!("../include/termy.h"));

    let missing_from_header = rust_structs
        .keys()
        .filter(|name| !header_structs.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let missing_from_rust = header_structs
        .keys()
        .filter(|name| !rust_structs.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let mismatches = rust_structs
        .iter()
        .filter_map(|(name, rust_fields)| {
            header_structs.get(name).and_then(|header_fields| {
                (header_fields != rust_fields)
                    .then(|| format!("{name}: rust={rust_fields:?}, header={header_fields:?}"))
            })
        })
        .collect::<Vec<_>>();

    assert!(
        missing_from_header.is_empty(),
        "Rust repr(C) structs missing from termy.h: {missing_from_header:?}"
    );
    assert!(
        missing_from_rust.is_empty(),
        "termy.h structs missing Rust repr(C) structs: {missing_from_rust:?}"
    );
    assert!(
        mismatches.is_empty(),
        "termy.h struct layouts drifted from Rust repr(C) types: {mismatches:#?}"
    );
    assert!(!rust_structs.is_empty(), "expected Rust repr(C) structs");
}

#[test]
fn c_header_enums_match_rust_repr_c_enums() {
    let rust_enums = rust_repr_c_enum_shapes(include_str!("../src/lib.rs"));
    let header_enums = header_enum_shapes(include_str!("../include/termy.h"));

    let missing_from_header = rust_enums
        .keys()
        .filter(|name| !header_enums.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let mismatches = rust_enums
        .iter()
        .filter_map(|(name, rust_enum)| {
            header_enums.get(name).and_then(|header_enum| {
                (header_enum != rust_enum)
                    .then(|| format!("{name}: rust={rust_enum:?}, header={header_enum:?}"))
            })
        })
        .collect::<Vec<_>>();

    assert!(
        missing_from_header.is_empty(),
        "Rust repr(C) enums missing from termy.h: {missing_from_header:?}"
    );
    assert!(
        mismatches.is_empty(),
        "termy.h enum values drifted from Rust repr(C) types: {mismatches:#?}"
    );
    assert!(!rust_enums.is_empty(), "expected Rust repr(C) enums");
}
