const AMF_MAX_DEPTH: usize = 16;

/// Minimal AMF0 number reader: returns Some(value, bytes_consumed).
pub fn amf_read_number(data: &[u8]) -> Option<(f64, usize)> {
    if data.len() < 9 || data[0] != 0x00 {
        return None;
    }
    let bits = u64::from_be_bytes(data[1..9].try_into().ok()?);
    Some((f64::from_bits(bits), 9))
}

/// AMF0 value parser: returns (marker, value_bytes, total_bytes_consumed).
/// Recursion is bounded by `AMF_MAX_DEPTH` to prevent stack overflow on
/// deeply nested or malicious payloads.
pub fn amf_next_value(data: &[u8]) -> Option<(u8, &[u8], usize)> {
    amf_next_value_inner(data, 0)
}

fn amf_next_value_inner(data: &[u8], depth: usize) -> Option<(u8, &[u8], usize)> {
    if depth > AMF_MAX_DEPTH {
        return None;
    }
    if data.is_empty() {
        return None;
    }
    let marker = data[0];
    match marker {
        0x00 => {
            // Number
            let (_, n) = amf_read_number(data)?;
            Some((marker, &data[1..9], n))
        }
        0x01 => {
            // Boolean
            Some((marker, &data[1..2], 2))
        }
        0x02 => {
            // String
            if data.len() < 3 {
                return None;
            }
            let len = u16::from_be_bytes([data[1], data[2]]) as usize;
            if data.len() < 3 + len {
                return None;
            }
            Some((marker, &data[3..3 + len], 3 + len))
        }
        0x05 | 0x06 => {
            // Null / Undefined
            Some((marker, &[], 1))
        }
        0x08 => {
            // ECMA Array
            if data.len() < 5 {
                return None;
            }
            let _count = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            let mut off = 5;
            loop {
                if off + 3 > data.len() {
                    return None;
                }
                let klen = u16::from_be_bytes([data[off], data[off + 1]]) as usize;
                if klen == 0 && off + 2 < data.len() && data[off + 2] == 0x09 {
                    off += 3;
                    break;
                }
                if off + 2 + klen >= data.len() {
                    return None;
                }
                off += 2 + klen;
                let val_marker = data[off];
                if val_marker == 0x00 {
                    off += 9;
                } else if val_marker == 0x01 {
                    off += 2;
                } else if val_marker == 0x02 {
                    let slen = u16::from_be_bytes([data[off + 1], data[off + 2]]) as usize;
                    off += 3 + slen;
                } else if val_marker == 0x03 || val_marker == 0x08 || val_marker == 0x0A {
                    let (_, _, n) = amf_next_value_inner(&data[off..], depth + 1)?;
                    off += n;
                } else if val_marker == 0x05 || val_marker == 0x06 {
                    off += 1;
                } else {
                    return None;
                }
            }
            Some((marker, &data[5..off], off))
        }
        0x03 => {
            // Object
            let mut off = 1;
            off = skip_amf_object_inner(data, off, depth + 1)?;
            Some((marker, &data[1..off], off))
        }
        0x0A => {
            // Strict Array
            if data.len() < 5 {
                return None;
            }
            let _count = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            let mut off = 5;
            let mut remaining = _count as usize;
            while remaining > 0 && off < data.len() {
                let (_, _, n) = amf_next_value_inner(&data[off..], depth + 1)?;
                off += n;
                remaining -= 1;
            }
            Some((marker, &data[5..off], off))
        }
        _ => None,
    }
}

/// Skip past an AMF0 Object (0x03): key-value pairs terminated by 0x0009.
fn skip_amf_object_inner(data: &[u8], start: usize, depth: usize) -> Option<usize> {
    let mut off = start;
    loop {
        if off + 3 > data.len() {
            return None;
        }
        let klen = u16::from_be_bytes([data[off], data[off + 1]]) as usize;
        if klen == 0 && off + 2 < data.len() && data[off + 2] == 0x09 {
            off += 3;
            break;
        }
        if off + 2 + klen >= data.len() {
            return None;
        }
        off += 2 + klen;
        let (_, _, n) = amf_next_value_inner(&data[off..], depth)?;
        off += n;
    }
    Some(off)
}

/// Look up a field by name inside an AMF0 ECMA Array or Object at the given offset.
pub fn amf_lookup<'a>(data: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let (_, _, total) = amf_next_value(data)?;
    let marker = data[0];
    let body = &data[1..total];
    if marker != 0x03 && marker != 0x08 {
        return None;
    }
    if marker == 0x08 {
        if body.len() < 4 {
            return None;
        }
        let mut off = 4;
        loop {
            if off + 2 > body.len() {
                return None;
            }
            let klen = u16::from_be_bytes([body[off], body[off + 1]]) as usize;
            if klen == 0 && off + 2 < body.len() && body[off + 2] == 0x09 {
                break;
            }
            if off + 2 + klen > body.len() {
                return None;
            }
            let key = std::str::from_utf8(&body[off + 2..off + 2 + klen]).ok();
            off += 2 + klen;
            if off >= body.len() {
                return None;
            }
            if key == Some(name) {
                let (_, _, vlen) = amf_next_value(&body[off..])?;
                return Some(&body[off..off + vlen]);
            }
            let (_, _, vlen) = amf_next_value(&body[off..])?;
            off += vlen;
        }
        None
    } else {
        let mut off = 0;
        loop {
            if off + 2 > body.len() {
                return None;
            }
            let klen = u16::from_be_bytes([body[off], body[off + 1]]) as usize;
            if klen == 0 && off + 2 < body.len() && body[off + 2] == 0x09 {
                break;
            }
            if off + 2 + klen > body.len() {
                return None;
            }
            let key = std::str::from_utf8(&body[off + 2..off + 2 + klen]).ok();
            off += 2 + klen;
            if off >= body.len() {
                return None;
            }
            if key == Some(name) {
                let (_, _, vlen) = amf_next_value(&body[off..])?;
                return Some(&body[off..off + vlen]);
            }
            let (_, _, vlen) = amf_next_value(&body[off..])?;
            off += vlen;
        }
        None
    }
}
