use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub fn hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

/// Read a big-endian u32 from data at the given offset.
/// Returns 0 if insufficient bytes remain at that offset.
fn read_u32_at(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

pub fn find_box(data: &[u8], box_type: &[u8; 4]) -> Option<usize> {
    find_box_in_range(data, box_type)
}

pub fn find_box_in_range(data: &[u8], box_type: &[u8; 4]) -> Option<usize> {
    let mut offset = 0;
    while offset + 8 <= data.len() {
        let size = read_u32_at(data, offset) as usize;
        if size == 0 {
            break;
        }
        if size == 1 {
            offset += 16;
            continue;
        }
        // Guard against malformed boxes: size too small or extends past buffer
        if size < 8 || offset + size > data.len() {
            break;
        }
        if &data[offset + 4..offset + 8] == box_type {
            return Some(offset);
        }
        offset += size;
    }
    None
}

pub fn read_u32(data: &[u8]) -> u32 {
    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

pub fn write_u32(data: &mut [u8], value: u32) {
    data[..4].copy_from_slice(&value.to_be_bytes());
}

pub fn read_u64(data: &[u8]) -> u64 {
    u64::from_be_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}

pub fn write_u64(data: &mut [u8], value: u64) {
    data[..8].copy_from_slice(&value.to_be_bytes());
}
