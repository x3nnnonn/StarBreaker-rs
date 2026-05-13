use std::io::Write;

use rustc_hash::FxHashMap;

use crate::database::Database;
use crate::enums::{ConversionType, DataType};
use crate::error::ExportError;
use crate::reader::SpanReader;
use crate::types::{CigGuid, Pointer, Record, Reference, StringId};
use crate::walker::prescan_weak_pointers;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

pub fn to_dataforge_xml(
    db: &Database,
    record: &Record,
    include_record_category: bool,
) -> Result<Vec<u8>, ExportError> {
    let mut buf = Vec::with_capacity(4096);
    write_dataforge_xml(db, record, include_record_category, &mut buf)?;
    Ok(buf)
}

pub fn write_dataforge_xml(
    db: &Database,
    record: &Record,
    include_record_category: bool,
    buf: &mut Vec<u8>,
) -> Result<(), ExportError> {
    buf.extend_from_slice(&UTF8_BOM);
    buf.extend_from_slice(b"<?xml version=\"1.0\" encoding=\"utf-8\"?>");

    let weak_pointers = prescan_weak_pointers(db, record);

    let path = db.resolve_string(record.file_name_offset);
    let record_name = db.resolve_string2(record.name_offset);
    let struct_name = db.resolve_string2(db.struct_def(record.struct_index).name_offset);

    let mut ctx = Ctx {
        db,
        weak_pointers: &weak_pointers,
        path,
        indent: 0,
    };

    write_newline_indent(buf, ctx.indent);
    buf.push(b'<');
    buf.extend_from_slice(record_name.as_bytes());
    write_attr_guid(buf, "RecordId", &record.id);
    if include_record_category {
        let tag = record.tag_offset.0;
        if tag >= 0 {
            let category = db.resolve_string2(record.tag_offset);
            if !category.trim().is_empty() {
                write_attr(buf, "RecordCategory", category);
            }
        }
    }
    write_attr_pointer_if_target(buf, ctx.weak_pointers, record.struct_index, record.instance_index as i32);
    write_attr(buf, "Type", struct_name);

    let has_fields = !db.all_property_indices(record.struct_index).is_empty();
    if !has_fields {
        buf.extend_from_slice(b" />");
        return Ok(());
    }
    buf.push(b'>');
    ctx.indent += 1;

    let instance_bytes = db.get_instance(record.struct_index, record.instance_index as i32);
    let mut reader = SpanReader::new(instance_bytes);
    walk_fields(&mut ctx, buf, record.struct_index, &mut reader)?;

    ctx.indent -= 1;
    write_newline_indent(buf, ctx.indent);
    buf.extend_from_slice(b"</");
    buf.extend_from_slice(record_name.as_bytes());
    buf.push(b'>');

    Ok(())
}

struct Ctx<'a> {
    db: &'a Database<'a>,
    weak_pointers: &'a FxHashMap<(i32, i32), usize>,
    path: &'a str,
    indent: usize,
}

fn walk_fields(
    ctx: &mut Ctx<'_>,
    buf: &mut Vec<u8>,
    struct_index: i32,
    reader: &mut SpanReader,
) -> Result<(), ExportError> {
    let property_defs = ctx.db.property_defs();
    let indices = ctx.db.all_property_indices(struct_index);
    for &idx in indices {
        let prop = &property_defs[idx as usize];
        let data_type = DataType::try_from(prop.data_type)?;
        let conv_type = ConversionType::try_from(prop.conversion_type)?;
        let name = ctx.db.resolve_string2(prop.name_offset);
        if conv_type == ConversionType::Attribute {
            write_attribute(ctx, buf, data_type, prop.struct_index as i32, name, reader)?;
        } else {
            write_array(ctx, buf, data_type, prop.struct_index as i32, name, reader)?;
        }
    }
    Ok(())
}

fn write_attribute(
    ctx: &mut Ctx<'_>,
    buf: &mut Vec<u8>,
    data_type: DataType,
    prop_struct_index: i32,
    name: &str,
    reader: &mut SpanReader,
) -> Result<(), ExportError> {
    match data_type {
        DataType::Reference => {
            let reference = reader.read_type::<Reference>()?;
            write_reference_element(ctx, buf, name, *reference)?;
        }
        DataType::WeakPointer => {
            let ptr = reader.read_type::<Pointer>()?;
            open_self_closing(buf, ctx.indent, name);
            if !ptr.is_null()
                && let Some(&id) = ctx.weak_pointers.get(&(ptr.struct_index, ptr.instance_index))
            {
                write_attr_pointer(buf, id, "PointsTo");
            }
            buf.extend_from_slice(b" />");
        }
        DataType::StrongPointer => {
            let ptr = reader.read_type::<Pointer>()?;
            if ptr.is_null() {
                open_self_closing(buf, ctx.indent, name);
                buf.extend_from_slice(b" />");
            } else {
                write_struct_container(ctx, buf, name, ptr.struct_index, ptr.instance_index, true)?;
            }
        }
        DataType::Class => {
            let struct_name = ctx
                .db
                .resolve_string2(ctx.db.struct_def(prop_struct_index).name_offset);
            let has_fields = !ctx.db.all_property_indices(prop_struct_index).is_empty();
            write_newline_indent(buf, ctx.indent);
            buf.push(b'<');
            buf.extend_from_slice(name.as_bytes());
            write_attr(buf, "Type", struct_name);
            if !has_fields {
                buf.extend_from_slice(b" />");
            } else {
                buf.push(b'>');
                ctx.indent += 1;
                walk_fields(ctx, buf, prop_struct_index, reader)?;
                ctx.indent -= 1;
                write_newline_indent(buf, ctx.indent);
                buf.extend_from_slice(b"</");
                buf.extend_from_slice(name.as_bytes());
                buf.push(b'>');
            }
        }
        DataType::EnumChoice => {
            let sid = reader.read_type::<StringId>()?;
            write_text_element(buf, ctx.indent, name, ctx.db.resolve_string(*sid));
        }
        DataType::Guid => {
            let guid = reader.read_type::<CigGuid>()?;
            write_newline_indent(buf, ctx.indent);
            buf.push(b'<');
            buf.extend_from_slice(name.as_bytes());
            buf.push(b'>');
            write_guid(buf, guid);
            buf.extend_from_slice(b"</");
            buf.extend_from_slice(name.as_bytes());
            buf.push(b'>');
        }
        DataType::Locale => {
            let sid = reader.read_type::<StringId>()?;
            write_text_element(buf, ctx.indent, name, ctx.db.resolve_string(*sid));
        }
        DataType::String => {
            let sid = reader.read_type::<StringId>()?;
            write_text_element(buf, ctx.indent, name, ctx.db.resolve_string(*sid));
        }
        DataType::Double => {
            let v = reader.read_f64()?;
            write_scalar(buf, ctx.indent, name, |b| write_g_double(b, v));
        }
        DataType::Single => {
            let v = reader.read_f32()?;
            write_scalar(buf, ctx.indent, name, |b| write_g_single(b, v));
        }
        DataType::UInt64 => {
            let v = reader.read_u64()?;
            write_scalar(buf, ctx.indent, name, |b| write_u64(b, v));
        }
        DataType::UInt32 => {
            let v = reader.read_u32()?;
            write_scalar(buf, ctx.indent, name, |b| write_u32(b, v));
        }
        DataType::UInt16 => {
            let v = reader.read_u16()?;
            write_scalar(buf, ctx.indent, name, |b| write_u32(b, v as u32));
        }
        DataType::Byte => {
            let v = reader.read_u8()?;
            write_scalar(buf, ctx.indent, name, |b| write_u32(b, v as u32));
        }
        DataType::Int64 => {
            let v = reader.read_i64()?;
            write_scalar(buf, ctx.indent, name, |b| write_i64(b, v));
        }
        DataType::Int32 => {
            let v = reader.read_i32()?;
            write_scalar(buf, ctx.indent, name, |b| write_i32(b, v));
        }
        DataType::Int16 => {
            let v = reader.read_i16()?;
            write_scalar(buf, ctx.indent, name, |b| write_i32(b, v as i32));
        }
        DataType::SByte => {
            let v = reader.read_i8()?;
            write_scalar(buf, ctx.indent, name, |b| write_i32(b, v as i32));
        }
        DataType::Boolean => {
            let v = reader.read_bool()?;
            write_text_element(buf, ctx.indent, name, if v { "True" } else { "False" });
        }
    }
    Ok(())
}

fn write_array(
    ctx: &mut Ctx<'_>,
    buf: &mut Vec<u8>,
    data_type: DataType,
    prop_struct_index: i32,
    name: &str,
    reader: &mut SpanReader,
) -> Result<(), ExportError> {
    let count = reader.read_i32()?;
    let first_index = reader.read_i32()?;
    let array_struct_name = ctx
        .db
        .resolve_string2(ctx.db.struct_def(prop_struct_index).name_offset);

    write_newline_indent(buf, ctx.indent);
    buf.push(b'<');
    buf.extend_from_slice(name.as_bytes());
    write_attr(buf, "Type", array_struct_name);
    write_attr_count(buf, count);

    if count == 0 {
        buf.extend_from_slice(b" />");
        return Ok(());
    }
    buf.push(b'>');
    ctx.indent += 1;

    for i in first_index..first_index + count {
        let idx = i as usize;
        match data_type {
            DataType::Reference => {
                let reference = ctx.db.reference_values[idx];
                let resolved = if reference.is_null() {
                    None
                } else {
                    ctx.db.record_by_id(&reference.record_id)
                };
                if let Some(record) = resolved {
                    let actual_struct = ctx
                        .db
                        .resolve_string2(ctx.db.struct_def(record.struct_index).name_offset);
                    write_newline_indent(buf, ctx.indent);
                    buf.push(b'<');
                    buf.extend_from_slice(actual_struct.as_bytes());
                    let has_inline = write_reference_attrs(buf, ctx, reference);
                    if has_inline {
                        write_attr_pointer_if_target(
                            buf,
                            ctx.weak_pointers,
                            record.struct_index,
                            record.instance_index as i32,
                        );
                        write_attr(buf, "Type", actual_struct);
                        let has_fields =
                            !ctx.db.all_property_indices(record.struct_index).is_empty();
                        if !has_fields {
                            buf.extend_from_slice(b" />");
                        } else {
                            buf.push(b'>');
                            ctx.indent += 1;
                            let inst_bytes = ctx
                                .db
                                .get_instance(record.struct_index, record.instance_index as i32);
                            let mut sub_reader = SpanReader::new(inst_bytes);
                            walk_fields(ctx, buf, record.struct_index, &mut sub_reader)?;
                            ctx.indent -= 1;
                            write_newline_indent(buf, ctx.indent);
                            buf.extend_from_slice(b"</");
                            buf.extend_from_slice(actual_struct.as_bytes());
                            buf.push(b'>');
                        }
                    } else {
                        buf.extend_from_slice(b" />");
                    }
                } else {
                    write_newline_indent(buf, ctx.indent);
                    buf.push(b'<');
                    buf.extend_from_slice(array_struct_name.as_bytes());
                    buf.extend_from_slice(b" />");
                }
            }
            DataType::WeakPointer => {
                let ptr = ctx.db.weak_values[idx];
                write_newline_indent(buf, ctx.indent);
                buf.push(b'<');
                if ptr.is_null() {
                    buf.extend_from_slice(array_struct_name.as_bytes());
                } else {
                    let struct_name = ctx
                        .db
                        .resolve_string2(ctx.db.struct_def(ptr.struct_index).name_offset);
                    buf.extend_from_slice(struct_name.as_bytes());
                    if let Some(&id) =
                        ctx.weak_pointers.get(&(ptr.struct_index, ptr.instance_index))
                    {
                        write_attr_pointer(buf, id, "PointsTo");
                    }
                }
                buf.extend_from_slice(b" />");
            }
            DataType::StrongPointer => {
                let ptr = ctx.db.strong_values[idx];
                if ptr.is_null() {
                    write_newline_indent(buf, ctx.indent);
                    buf.push(b'<');
                    buf.extend_from_slice(array_struct_name.as_bytes());
                    buf.extend_from_slice(b" />");
                } else {
                    let struct_name = ctx
                        .db
                        .resolve_string2(ctx.db.struct_def(ptr.struct_index).name_offset);
                    write_struct_container(
                        ctx,
                        buf,
                        struct_name,
                        ptr.struct_index,
                        ptr.instance_index,
                        true,
                    )?;
                }
            }
            DataType::Class => {
                write_struct_container(ctx, buf, array_struct_name, prop_struct_index, i, true)?;
            }
            DataType::EnumChoice => {
                let sid = ctx.db.enum_values[idx];
                write_text_element(buf, ctx.indent, "EnumChoice", ctx.db.resolve_string(sid));
            }
            DataType::Guid => {
                let g = &ctx.db.guid_values[idx];
                write_newline_indent(buf, ctx.indent);
                buf.extend_from_slice(b"<Guid>");
                write_guid(buf, g);
                buf.extend_from_slice(b"</Guid>");
            }
            DataType::Locale => {
                let sid = ctx.db.locale_values[idx];
                write_text_element(buf, ctx.indent, "Locale", ctx.db.resolve_string(sid));
            }
            DataType::String => {
                let sid = ctx.db.string_id_values[idx];
                write_text_element(buf, ctx.indent, "String", ctx.db.resolve_string(sid));
            }
            DataType::Double => {
                let v = ctx.db.get_double(idx)?;
                write_scalar(buf, ctx.indent, "Double", |b| write_g_double(b, v));
            }
            DataType::Single => {
                let v = ctx.db.get_single(idx)?;
                write_scalar(buf, ctx.indent, "Single", |b| write_g_single(b, v));
            }
            DataType::UInt64 => {
                let v = ctx.db.get_uint64(idx)?;
                write_scalar(buf, ctx.indent, "UInt64", |b| write_u64(b, v));
            }
            DataType::UInt32 => {
                let v = ctx.db.get_uint32(idx)?;
                write_scalar(buf, ctx.indent, "UInt32", |b| write_u32(b, v));
            }
            DataType::UInt16 => {
                let v = ctx.db.get_uint16(idx)?;
                write_scalar(buf, ctx.indent, "UInt16", |b| write_u32(b, v as u32));
            }
            DataType::Byte => {
                let v = ctx.db.get_uint8(idx)?;
                write_scalar(buf, ctx.indent, "Byte", |b| write_u32(b, v as u32));
            }
            DataType::Int64 => {
                let v = ctx.db.get_int64(idx)?;
                write_scalar(buf, ctx.indent, "Int64", |b| write_i64(b, v));
            }
            DataType::Int32 => {
                let v = ctx.db.get_int32(idx)?;
                write_scalar(buf, ctx.indent, "Int32", |b| write_i32(b, v));
            }
            DataType::Int16 => {
                let v = ctx.db.get_int16(idx)?;
                write_scalar(buf, ctx.indent, "Int16", |b| write_i32(b, v as i32));
            }
            DataType::SByte => {
                let v = ctx.db.get_int8(idx)?;
                write_scalar(buf, ctx.indent, "SByte", |b| write_i32(b, v as i32));
            }
            DataType::Boolean => {
                let v = ctx.db.get_bool(idx)?;
                write_text_element(buf, ctx.indent, "Boolean", if v { "True" } else { "False" });
            }
        }
    }

    ctx.indent -= 1;
    write_newline_indent(buf, ctx.indent);
    buf.extend_from_slice(b"</");
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'>');
    Ok(())
}

fn write_reference_element(
    ctx: &mut Ctx<'_>,
    buf: &mut Vec<u8>,
    name: &str,
    reference: Reference,
) -> Result<(), ExportError> {
    write_newline_indent(buf, ctx.indent);
    buf.push(b'<');
    buf.extend_from_slice(name.as_bytes());
    let inline = write_reference_attrs(buf, ctx, reference);
    if inline {
        let target = ctx.db.record_by_id(&reference.record_id).unwrap();
        let target_struct = ctx
            .db
            .resolve_string2(ctx.db.struct_def(target.struct_index).name_offset);
        write_attr_pointer_if_target(
            buf,
            ctx.weak_pointers,
            target.struct_index,
            target.instance_index as i32,
        );
        write_attr(buf, "Type", target_struct);
        let has_fields = !ctx.db.all_property_indices(target.struct_index).is_empty();
        if !has_fields {
            buf.extend_from_slice(b" />");
        } else {
            buf.push(b'>');
            ctx.indent += 1;
            let inst = ctx
                .db
                .get_instance(target.struct_index, target.instance_index as i32);
            let mut r = SpanReader::new(inst);
            walk_fields(ctx, buf, target.struct_index, &mut r)?;
            ctx.indent -= 1;
            write_newline_indent(buf, ctx.indent);
            buf.extend_from_slice(b"</");
            buf.extend_from_slice(name.as_bytes());
            buf.push(b'>');
        }
    } else {
        buf.extend_from_slice(b" />");
    }
    Ok(())
}

fn write_reference_attrs(buf: &mut Vec<u8>, ctx: &Ctx<'_>, reference: Reference) -> bool {
    if reference.is_null() {
        return false;
    }
    let Some(target) = ctx.db.record_by_id(&reference.record_id) else {
        return false;
    };
    let target_path = ctx.db.resolve_string(target.file_name_offset);
    let target_name = ctx.db.resolve_string2(target.name_offset);

    if ctx.db.is_main_record(target) {
        write_attr_relpath(buf, "ReferencedFile", target_path, ctx.path);
        return false;
    }
    if target_path == ctx.path {
        write_attr_guid(buf, "RecordId", &reference.record_id);
        write_attr(buf, "RecordName", target_name);
        return true;
    }
    write_attr_relpath(buf, "RecordReference", target_path, ctx.path);
    write_attr(buf, "RecordName", target_name);
    write_attr_guid(buf, "RecordId", &reference.record_id);
    false
}

fn write_struct_container(
    ctx: &mut Ctx<'_>,
    buf: &mut Vec<u8>,
    element_name: &str,
    struct_index: i32,
    instance_index: i32,
    check_pointer: bool,
) -> Result<(), ExportError> {
    let struct_name = ctx
        .db
        .resolve_string2(ctx.db.struct_def(struct_index).name_offset);
    write_newline_indent(buf, ctx.indent);
    buf.push(b'<');
    buf.extend_from_slice(element_name.as_bytes());
    if check_pointer {
        write_attr_pointer_if_target(buf, ctx.weak_pointers, struct_index, instance_index);
    }
    write_attr(buf, "Type", struct_name);

    let has_fields = !ctx.db.all_property_indices(struct_index).is_empty();
    if !has_fields {
        buf.extend_from_slice(b" />");
        return Ok(());
    }
    buf.push(b'>');
    ctx.indent += 1;
    let inst = ctx.db.get_instance(struct_index, instance_index);
    let mut r = SpanReader::new(inst);
    walk_fields(ctx, buf, struct_index, &mut r)?;
    ctx.indent -= 1;
    write_newline_indent(buf, ctx.indent);
    buf.extend_from_slice(b"</");
    buf.extend_from_slice(element_name.as_bytes());
    buf.push(b'>');
    Ok(())
}

fn write_newline_indent(buf: &mut Vec<u8>, indent: usize) {
    buf.extend_from_slice(b"\r\n");
    for _ in 0..indent {
        buf.extend_from_slice(b"  ");
    }
}

fn open_self_closing(buf: &mut Vec<u8>, indent: usize, name: &str) {
    write_newline_indent(buf, indent);
    buf.push(b'<');
    buf.extend_from_slice(name.as_bytes());
}

fn write_text_element(buf: &mut Vec<u8>, indent: usize, name: &str, value: &str) {
    write_newline_indent(buf, indent);
    buf.push(b'<');
    buf.extend_from_slice(name.as_bytes());
    if value.is_empty() {
        buf.extend_from_slice(b" />");
        return;
    }
    buf.push(b'>');
    escape_text(buf, value);
    buf.extend_from_slice(b"</");
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'>');
}

#[inline]
fn write_scalar<F: FnOnce(&mut Vec<u8>)>(buf: &mut Vec<u8>, indent: usize, name: &str, f: F) {
    write_newline_indent(buf, indent);
    buf.push(b'<');
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'>');
    f(buf);
    buf.extend_from_slice(b"</");
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'>');
}

fn write_attr(buf: &mut Vec<u8>, key: &str, value: &str) {
    buf.push(b' ');
    buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(b"=\"");
    escape_attr(buf, value);
    buf.push(b'"');
}

fn write_attr_guid(buf: &mut Vec<u8>, key: &str, guid: &CigGuid) {
    buf.push(b' ');
    buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(b"=\"");
    write_guid(buf, guid);
    buf.push(b'"');
}

fn write_attr_count(buf: &mut Vec<u8>, count: i32) {
    buf.extend_from_slice(b" Count=\"");
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(count).as_bytes());
    buf.push(b'"');
}

fn write_attr_pointer(buf: &mut Vec<u8>, id: usize, key: &str) {
    buf.push(b' ');
    buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(b"=\"ptr:");
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(id).as_bytes());
    buf.push(b'"');
}

fn write_attr_pointer_if_target(
    buf: &mut Vec<u8>,
    wp: &FxHashMap<(i32, i32), usize>,
    struct_index: i32,
    instance_index: i32,
) {
    if let Some(&id) = wp.get(&(struct_index, instance_index)) {
        write_attr_pointer(buf, id, "Pointer");
    }
}

fn write_attr_relpath(buf: &mut Vec<u8>, key: &str, target: &str, context: &str) {
    buf.push(b' ');
    buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(b"=\"file://./");
    let slashes = context.matches('/').count();
    for _ in 0..slashes {
        buf.extend_from_slice(b"../");
    }
    escape_attr(buf, target);
    buf.push(b'"');
}

fn escape_attr(buf: &mut Vec<u8>, s: &str) {
    for &b in s.as_bytes() {
        match b {
            b'&' => buf.extend_from_slice(b"&amp;"),
            b'<' => buf.extend_from_slice(b"&lt;"),
            b'>' => buf.extend_from_slice(b"&gt;"),
            b'"' => buf.extend_from_slice(b"&quot;"),
            _ => buf.push(b),
        }
    }
}

fn escape_text(buf: &mut Vec<u8>, s: &str) {
    for &b in s.as_bytes() {
        match b {
            b'&' => buf.extend_from_slice(b"&amp;"),
            b'<' => buf.extend_from_slice(b"&lt;"),
            _ => buf.push(b),
        }
    }
}

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(v).as_bytes());
}

fn write_u64(buf: &mut Vec<u8>, v: u64) {
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(v).as_bytes());
}

fn write_i32(buf: &mut Vec<u8>, v: i32) {
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(v).as_bytes());
}

fn write_i64(buf: &mut Vec<u8>, v: i64) {
    let mut tmp = itoa::Buffer::new();
    buf.extend_from_slice(tmp.format(v).as_bytes());
}

fn write_guid(buf: &mut Vec<u8>, guid: &CigGuid) {
    let _ = write!(buf, "{guid}");
}

fn write_g_single(buf: &mut Vec<u8>, v: f32) {
    if v == 0.0 {
        buf.push(b'0');
        return;
    }
    if v.is_nan() {
        buf.extend_from_slice(b"NaN");
        return;
    }
    if v.is_infinite() {
        buf.extend_from_slice(if v > 0.0 { b"Infinity" } else { b"-Infinity" });
        return;
    }
    let mut tmp = ryu::Buffer::new();
    let sci = tmp.format(v);
    write_g_from_sci(buf, sci, 9);
}

fn write_g_double(buf: &mut Vec<u8>, v: f64) {
    if v == 0.0 {
        buf.push(b'0');
        return;
    }
    if v.is_nan() {
        buf.extend_from_slice(b"NaN");
        return;
    }
    if v.is_infinite() {
        buf.extend_from_slice(if v > 0.0 { b"Infinity" } else { b"-Infinity" });
        return;
    }
    let mut tmp = ryu::Buffer::new();
    let sci = tmp.format(v);
    write_g_from_sci(buf, sci, 17);
}

fn write_g_from_sci(buf: &mut Vec<u8>, sci: &str, precision: i32) {
    let bytes = sci.as_bytes();
    let (is_negative, after_sign) = if bytes[0] == b'-' {
        (true, &sci[1..])
    } else {
        (false, sci)
    };

    let (mant_str, exp_str) = match after_sign.split_once('e') {
        Some(t) => t,
        None => (after_sign, "0"),
    };
    let exp: i32 = exp_str.parse().unwrap_or(0);

    let (int_part, frac_part) = match mant_str.split_once('.') {
        Some((a, b)) => (a, b),
        None => (mant_str, ""),
    };

    let mut digits: Vec<u8> = int_part.bytes().chain(frac_part.bytes()).collect();
    let true_exp = if int_part == "0" {
        let leading = frac_part.bytes().take_while(|&b| b == b'0').count();
        digits.drain(..1 + leading);
        -(leading as i32 + 1) + exp
    } else {
        (int_part.len() as i32 - 1) + exp
    };

    while digits.len() > 1 && *digits.last().unwrap() == b'0' {
        digits.pop();
    }

    if is_negative {
        buf.push(b'-');
    }

    if true_exp < -4 || true_exp >= precision {
        buf.push(digits[0]);
        if digits.len() > 1 {
            buf.push(b'.');
            buf.extend_from_slice(&digits[1..]);
        }
        buf.push(b'E');
        buf.push(if true_exp >= 0 { b'+' } else { b'-' });
        let abs_exp = true_exp.unsigned_abs();
        if abs_exp < 10 {
            buf.push(b'0');
            buf.push(b'0' + abs_exp as u8);
        } else {
            let mut tmp = itoa::Buffer::new();
            buf.extend_from_slice(tmp.format(abs_exp).as_bytes());
        }
    } else if true_exp >= 0 {
        let n_int = (true_exp + 1) as usize;
        if digits.len() <= n_int {
            buf.extend_from_slice(&digits);
            for _ in digits.len()..n_int {
                buf.push(b'0');
            }
        } else {
            buf.extend_from_slice(&digits[..n_int]);
            buf.push(b'.');
            buf.extend_from_slice(&digits[n_int..]);
        }
    } else {
        buf.push(b'0');
        buf.push(b'.');
        for _ in 0..(-true_exp - 1) {
            buf.push(b'0');
        }
        buf.extend_from_slice(&digits);
    }
}
