use super::{AudioCodec, Fmp4Muxer, VideoCodec};

// ── RFC 6381 codec strings ───────────────────────────────────────

impl Fmp4Muxer {
    pub fn codec_string(&self) -> Option<String> {
        let v = self.video_codec_string()?;
        let a = self.audio_codec_string()?;
        Some(format!("{},{}", v, a))
    }

    pub fn video_codec_string(&self) -> Option<String> {
        let codec = self.video_codec?;
        match codec {
            VideoCodec::H264 => {
                let config = self.video_config.as_ref()?;
                if config.len() < 4 {
                    return None;
                }
                Some(format!(
                    "avc1.{:02X}{:02X}{:02X}",
                    config[1], config[2], config[3]
                ))
            }
            VideoCodec::H265 => {
                let config = self.video_config.as_ref()?;
                if config.len() < 13 {
                    return None;
                }
                let tier_flag = (config[1] >> 5) & 0x01;
                let profile_idc = config[1] & 0x1F;
                let profile_compatibility = Self::reverse_bits_32(u32::from_be_bytes([
                    config[2], config[3], config[4], config[5],
                ]));
                let constraints = &config[6..12];
                let level_idc = config[12];
                let tier = if tier_flag == 1 { 'H' } else { 'L' };

                let pc_str = if profile_compatibility != 0 {
                    format!("{:x}", profile_compatibility)
                } else {
                    "0".to_string()
                };

                let constraints_str = constraints
                    .iter()
                    .rev()
                    .skip_while(|&&b| b == 0)
                    .collect::<Vec<_>>()
                    .iter()
                    .rev()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(".");

                let base = format!("hvc1.{}.{}.{}{}", profile_idc, pc_str, tier, level_idc);
                if constraints_str.is_empty() {
                    Some(base)
                } else {
                    Some(format!("{}.{}", base, constraints_str))
                }
            }
            VideoCodec::AV1 => {
                let config = self.video_config.as_ref()?;
                let av1c = if !config.is_empty() && config[0] == 0x81 {
                    config.to_vec()
                } else {
                    av1c_box_from_config(config)
                };
                if av1c.len() < 4 {
                    return None;
                }
                let profile = (av1c[1] >> 5) & 0x07;
                let level_idx = av1c[1] & 0x1F;
                let seq_tier_0 = (av1c[2] >> 7) & 0x01;
                let high_bitdepth = (av1c[2] >> 6) & 0x01;
                let twelve_bit = (av1c[2] >> 5) & 0x01;
                let bit_depth = if profile == 2 && high_bitdepth == 1 {
                    if twelve_bit == 1 { 12 } else { 10 }
                } else if profile <= 2 {
                    if high_bitdepth == 1 { 10 } else { 8 }
                } else {
                    8
                };
                let tier = if seq_tier_0 == 1 { 'H' } else { 'M' };
                Some(format!(
                    "av01.{}.{:02}{}{}",
                    profile, level_idx, tier, bit_depth
                ))
            }
        }
    }

    fn audio_codec_string(&self) -> Option<String> {
        let codec = self.audio_codec?;
        match codec {
            AudioCodec::Aac => Some("mp4a.40.2".to_string()),
            AudioCodec::Opus => Some("opus".to_string()),
            AudioCodec::Flac => Some("fLaC".to_string()),
        }
    }

    fn reverse_bits_32(x: u32) -> u32 {
        let mut v = x;
        v = ((v & 0x55555555) << 1) | ((v >> 1) & 0x55555555);
        v = ((v & 0x33333333) << 2) | ((v >> 2) & 0x33333333);
        v = ((v & 0x0F0F0F0F) << 4) | ((v >> 4) & 0x0F0F0F0F);
        v = ((v & 0x00FF00FF) << 8) | ((v >> 8) & 0x00FF00FF);
        v.rotate_right(16)
    }
}

// ── AV1 OBU helpers ─────────────────────────────────────────────

struct BitReader<'a> {
    data: &'a [u8],
    byte_offset: usize,
    bit_offset: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_offset: 0,
            bit_offset: 0,
        }
    }

    fn read_bit(&mut self) -> u8 {
        if self.byte_offset >= self.data.len() {
            return 0;
        }
        let bit = (self.data[self.byte_offset] >> (7 - self.bit_offset)) & 1;
        self.bit_offset += 1;
        if self.bit_offset == 8 {
            self.bit_offset = 0;
            self.byte_offset += 1;
        }
        bit
    }

    fn read_bits(&mut self, n: u8) -> u64 {
        let mut result = 0u64;
        for _ in 0..n {
            result = (result << 1) | self.read_bit() as u64;
        }
        result
    }
}

#[derive(Debug, Clone, Default)]
struct Av1SeqHeader {
    seq_profile: u8,
    seq_level_idx_0: u8,
    seq_tier_0: u8,
    high_bitdepth: u8,
    twelve_bit: u8,
    monochrome: u8,
    chroma_subsampling_x: u8,
    chroma_subsampling_y: u8,
    chroma_sample_position: u8,
    color_primaries: u16,
    transfer_characteristics: u16,
    matrix_coefficients: u16,
    full_range: bool,
}

fn parse_av1_sequence_header(payload: &[u8]) -> Option<Av1SeqHeader> {
    let mut r = BitReader::new(payload);
    if payload.len() < 2 {
        return None;
    }

    let seq_profile = r.read_bits(3) as u8;
    let mut h = Av1SeqHeader {
        seq_profile,
        ..Default::default()
    };
    let _still_picture = r.read_bit();
    let reduced_still_picture_header = r.read_bit();

    let mut decoder_model_info_present_flag = 0u8;
    let initial_display_delay_present_flag;
    let mut buffer_delay_length_minus_1 = 0u8;

    if reduced_still_picture_header == 1 {
        h.seq_level_idx_0 = r.read_bits(5) as u8;
        h.seq_tier_0 = 0;
    } else {
        let timing_info_present_flag = r.read_bit();
        if timing_info_present_flag == 1 {
            let _ = r.read_bits(32);
            let _ = r.read_bits(32);
            let equal_picture_interval = r.read_bit();
            if equal_picture_interval == 0 {
                let mut leading_zeros = 0u8;
                while r.read_bit() == 0 && leading_zeros < 31 {
                    leading_zeros += 1;
                }
                if leading_zeros > 0 && leading_zeros < 31 {
                    let _ = r.read_bits(leading_zeros);
                }
            }
            decoder_model_info_present_flag = r.read_bit();
            if decoder_model_info_present_flag == 1 {
                buffer_delay_length_minus_1 = r.read_bits(5) as u8;
                let _ = r.read_bits(32);
                let _ = r.read_bits(5);
                let _ = r.read_bits(5);
            }
        }
        initial_display_delay_present_flag = r.read_bit();
        let operating_points_cnt_minus_1 = r.read_bits(5) as u8;
        for i in 0..=operating_points_cnt_minus_1 {
            let _ = r.read_bits(12);
            let seq_level_idx = r.read_bits(5) as u8;
            let mut seq_tier = 0u8;
            if seq_level_idx > 7 {
                seq_tier = r.read_bit();
            }
            if i == 0 {
                h.seq_level_idx_0 = seq_level_idx;
                h.seq_tier_0 = seq_tier;
            }
            if decoder_model_info_present_flag == 1 {
                let decoder_model_present_for_this_op = r.read_bit();
                if decoder_model_present_for_this_op == 1 {
                    let n = buffer_delay_length_minus_1 + 1;
                    let _ = r.read_bits(n);
                    let _ = r.read_bits(n);
                    let _ = r.read_bit();
                }
            }
            if initial_display_delay_present_flag == 1 {
                let initial_display_delay_present_for_this_op = r.read_bit();
                if initial_display_delay_present_for_this_op == 1 {
                    let _ = r.read_bits(4);
                }
            }
        }
    }

    let frame_width_bits_minus_1 = r.read_bits(4) as u8;
    let frame_height_bits_minus_1 = r.read_bits(4) as u8;
    let n = frame_width_bits_minus_1 + 1;
    let _ = r.read_bits(n);
    let n = frame_height_bits_minus_1 + 1;
    let _ = r.read_bits(n);

    let frame_id_numbers_present_flag = if reduced_still_picture_header == 1 {
        0
    } else {
        r.read_bit()
    };
    if frame_id_numbers_present_flag == 1 {
        let _ = r.read_bits(4);
        let _ = r.read_bits(3);
    }

    let _ = r.read_bit();
    let _ = r.read_bit();
    let _ = r.read_bit();

    if reduced_still_picture_header == 0 {
        let _ = r.read_bit();
        let _ = r.read_bit();
        let _ = r.read_bit();
        let _ = r.read_bit();
        let _ = r.read_bit();
        let enable_order_hint = r.read_bit();
        if enable_order_hint == 1 {
            let _ = r.read_bit();
            let _ = r.read_bit();
        }
        let seq_choose_screen_content_tools = r.read_bit();
        let seq_force_screen_content_tools = if seq_choose_screen_content_tools == 1 {
            2
        } else {
            r.read_bit()
        };
        if seq_force_screen_content_tools > 0 {
            let seq_choose_integer_mv = r.read_bit();
            if seq_choose_integer_mv == 0 {
                let _ = r.read_bit();
            }
        }
        if enable_order_hint == 1 {
            let _ = r.read_bits(3);
        }
    }

    let _ = r.read_bit();
    let _ = r.read_bit();
    let _ = r.read_bit();

    h.high_bitdepth = r.read_bit();
    let bit_depth = if h.seq_profile == 2 && h.high_bitdepth == 1 {
        h.twelve_bit = r.read_bit();
        if h.twelve_bit == 1 { 12 } else { 10 }
    } else if h.seq_profile <= 2 {
        if h.high_bitdepth == 1 { 10 } else { 8 }
    } else {
        8
    };

    if h.seq_profile == 1 {
        h.monochrome = 0;
    } else {
        h.monochrome = r.read_bit();
    }

    let color_description_present_flag = r.read_bit();
    let mut color_primaries = 0u16;
    let mut transfer_characteristics = 0u16;
    let mut matrix_coefficients = 0u16;
    if color_description_present_flag == 1 {
        color_primaries = r.read_bits(8) as u16;
        transfer_characteristics = r.read_bits(8) as u16;
        matrix_coefficients = r.read_bits(8) as u16;
    }
    h.color_primaries = color_primaries;
    h.transfer_characteristics = transfer_characteristics;
    h.matrix_coefficients = matrix_coefficients;

    if h.monochrome == 1 {
        h.full_range = r.read_bit() == 1;
        h.chroma_subsampling_x = 1;
        h.chroma_subsampling_y = 1;
        h.chroma_sample_position = 0;
    } else if color_primaries == 1 && transfer_characteristics == 13 && matrix_coefficients == 0 {
        h.full_range = true;
        h.chroma_subsampling_x = 0;
        h.chroma_subsampling_y = 0;
        h.chroma_sample_position = 0;
    } else {
        h.full_range = r.read_bit() == 1;
        if h.seq_profile == 0 {
            h.chroma_subsampling_x = 1;
            h.chroma_subsampling_y = 1;
        } else if h.seq_profile == 1 {
            h.chroma_subsampling_x = 0;
            h.chroma_subsampling_y = 0;
        } else {
            if bit_depth == 12 {
                h.chroma_subsampling_x = r.read_bit();
                if h.chroma_subsampling_x == 1 {
                    h.chroma_subsampling_y = r.read_bit();
                } else {
                    h.chroma_subsampling_y = 0;
                }
            } else {
                h.chroma_subsampling_x = 1;
                h.chroma_subsampling_y = 0;
            }
        }
        if h.chroma_subsampling_x == 1 && h.chroma_subsampling_y == 1 {
            h.chroma_sample_position = r.read_bits(2) as u8;
        }
    }

    Some(h)
}

pub fn ensure_av1_obu_size_fields(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut offset = 0;
    let mut needs_rewrite = false;

    let mut scan = 0;
    while scan < data.len() {
        let obu_header = data[scan];
        let obu_extension_flag = (obu_header >> 2) & 1;
        let obu_has_size_field = (obu_header >> 1) & 1;
        let header_size = 1 + if obu_extension_flag == 1 { 1 } else { 0 };

        if obu_has_size_field == 1 {
            let size_start = scan + header_size;
            let mut size = 0usize;
            let mut shift = 0;
            let mut size_bytes = 0;
            loop {
                if size_start + size_bytes >= data.len() {
                    return data.to_vec();
                }
                let byte = data[size_start + size_bytes];
                size_bytes += 1;
                size |= ((byte & 0x7F) as usize) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 56 {
                    return data.to_vec();
                }
            }
            let new_scan = size_start.saturating_add(size_bytes).saturating_add(size);
            if new_scan > data.len() || new_scan <= scan {
                return data.to_vec();
            }
            scan = new_scan;
        } else {
            needs_rewrite = true;
            break;
        }
    }

    if !needs_rewrite {
        return data.to_vec();
    }

    while offset < data.len() {
        let obu_header = data[offset];
        let obu_extension_flag = (obu_header >> 2) & 1;
        let obu_has_size_field = (obu_header >> 1) & 1;
        let header_size = 1 + if obu_extension_flag == 1 { 1 } else { 0 };

        if obu_has_size_field == 1 {
            let size_start = offset + header_size;
            let mut size = 0usize;
            let mut shift = 0;
            let mut size_bytes = 0;
            loop {
                if size_start + size_bytes >= data.len() {
                    return data.to_vec();
                }
                let byte = data[size_start + size_bytes];
                size_bytes += 1;
                size |= ((byte & 0x7F) as usize) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 56 {
                    return data.to_vec();
                }
            }
            let obu_end = size_start.saturating_add(size_bytes).saturating_add(size);
            if obu_end > data.len() {
                return data.to_vec();
            }
            result.extend_from_slice(&data[offset..obu_end]);
            offset = obu_end;
        } else {
            let payload_start = offset + header_size;
            let payload_size = data.len() - payload_start;
            result.push(obu_header | 0x02);
            if obu_extension_flag == 1 {
                result.push(data[offset + 1]);
            }
            let mut sz = payload_size;
            loop {
                let mut byte = (sz & 0x7F) as u8;
                sz >>= 7;
                if sz != 0 {
                    byte |= 0x80;
                }
                result.push(byte);
                if sz == 0 {
                    break;
                }
            }
            result.extend_from_slice(&data[payload_start..]);
            offset = data.len();
        }
    }

    result
}

pub fn av1c_box_from_config(config: &[u8]) -> Vec<u8> {
    if config.is_empty() {
        return vec![0x81, 0x00, 0x00, 0x00];
    }
    if config[0] == 0x81 {
        return config.to_vec();
    }

    let mut offset = 0;
    let mut seq_header_payload: Option<&[u8]> = None;

    while offset < config.len() && seq_header_payload.is_none() {
        if offset >= config.len() {
            break;
        }
        let obu_header = config[offset];
        offset += 1;
        let obu_type = (obu_header >> 3) & 0x0F;
        let obu_extension_flag = (obu_header >> 2) & 1;
        let obu_has_size_field = (obu_header >> 1) & 1;

        if obu_extension_flag == 1 && offset < config.len() {
            offset += 1;
        }

        let mut obu_size = 0usize;
        if obu_has_size_field == 1 {
            let mut shift = 0;
            loop {
                if offset >= config.len() {
                    break;
                }
                let byte = config[offset];
                offset += 1;
                obu_size |= ((byte & 0x7F) as usize) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
        } else {
            obu_size = config.len().saturating_sub(offset);
        }

        if obu_type == 1 && offset + obu_size <= config.len() {
            seq_header_payload = Some(&config[offset..offset + obu_size]);
            break;
        }

        offset += obu_size;
    }

    let h = seq_header_payload.and_then(parse_av1_sequence_header);

    let profile_level = (h.as_ref().map(|s| s.seq_profile).unwrap_or(0) << 5)
        | (h.as_ref().map(|s| s.seq_level_idx_0).unwrap_or(0) & 0x1F);
    let flags2 = (h.as_ref().map(|s| s.seq_tier_0).unwrap_or(0) << 7)
        | (h.as_ref().map(|s| s.high_bitdepth).unwrap_or(0) << 6)
        | (h.as_ref().map(|s| s.twelve_bit).unwrap_or(0) << 5)
        | (h.as_ref().map(|s| s.monochrome).unwrap_or(0) << 4)
        | (h.as_ref().map(|s| s.chroma_subsampling_x).unwrap_or(0) << 3)
        | (h.as_ref().map(|s| s.chroma_subsampling_y).unwrap_or(0) << 2)
        | (h.as_ref().map(|s| s.chroma_sample_position).unwrap_or(0) & 0x03);

    let mut av1c = vec![0x81, profile_level, flags2, 0x00];
    let config_with_sizes = ensure_av1_obu_size_fields(config);
    av1c.extend_from_slice(&config_with_sizes);
    av1c
}

/// Extract color config from an AV1 codec configuration blob.
/// Scans for the sequence header OBU, parses it, and returns color info.
/// Returns None if no color description is present.
pub fn av1_color_config_from_config(config: &[u8]) -> Option<crate::hls::fmp4::ColorConfig> {
    // Normalize OBU size fields first (same as av1c_box_from_config)
    let config_with_sizes = crate::hls::fmp4::codec::ensure_av1_obu_size_fields(config);
    let config = &config_with_sizes;

    let mut offset = 0;
    let mut seq_header_payload: Option<&[u8]> = None;

    while offset < config.len() && seq_header_payload.is_none() {
        if offset >= config.len() {
            break;
        }
        let obu_header = config[offset];
        offset += 1;
        let obu_type = (obu_header >> 3) & 0x0F;
        let obu_extension_flag = (obu_header >> 2) & 1;
        let obu_has_size_field = (obu_header >> 1) & 1;

        if obu_extension_flag == 1 && offset < config.len() {
            offset += 1;
        }

        let mut obu_size = 0usize;
        if obu_has_size_field == 1 {
            let mut shift = 0;
            loop {
                if offset >= config.len() {
                    break;
                }
                let byte = config[offset];
                offset += 1;
                obu_size |= ((byte & 0x7F) as usize) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
        } else {
            obu_size = config.len().saturating_sub(offset);
        }

        if obu_type == 1 && offset + obu_size <= config.len() {
            seq_header_payload = Some(&config[offset..offset + obu_size]);
            break;
        }

        offset += obu_size;
    }

    let h = seq_header_payload.and_then(parse_av1_sequence_header)?;
    // Return color info if any field is meaningful (encoder may set only matrix_coefficients)
    if h.color_primaries == 0 && h.transfer_characteristics == 0 && h.matrix_coefficients == 0 {
        return None;
    }
    Some(crate::hls::fmp4::ColorConfig {
        color_primaries: h.color_primaries,
        transfer_characteristics: h.transfer_characteristics,
        matrix_coefficients: h.matrix_coefficients,
        full_range: h.full_range,
    })
}

// ── Audio config builders ────────────────────────────────────────

pub fn build_esds(audio_specific_config: &[u8]) -> Vec<u8> {
    let write_exp_len = |w: &mut Vec<u8>, val: usize, bytes: usize| {
        let mut v = val;
        let mut buf = vec![0u8; bytes];
        for i in (0..bytes).rev() {
            buf[i] = (v & 0x7F) as u8 | 0x80;
            v >>= 7;
        }
        buf[bytes - 1] &= 0x7F;
        w.extend_from_slice(&buf);
    };

    let dsi_len = audio_specific_config.len();
    let mut esds = Vec::new();

    esds.push(0x03);
    let dcd_body_len = 1 + 1 + 3 + 4 + 4 + 1 + 4 + dsi_len;
    let es_data_len = 2 + 1 + 1 + 4 + dcd_body_len + 1 + 4 + 1;
    write_exp_len(&mut esds, es_data_len, 4);
    esds.extend_from_slice(&1u16.to_be_bytes());
    esds.push(0x00);

    esds.push(0x04);
    write_exp_len(&mut esds, dcd_body_len, 4);
    esds.push(0x40);
    esds.push(0x15);
    esds.extend_from_slice(&[0u8; 3]);
    esds.extend_from_slice(&128000u32.to_be_bytes());
    esds.extend_from_slice(&128000u32.to_be_bytes());

    esds.push(0x05);
    write_exp_len(&mut esds, dsi_len, 4);
    esds.extend_from_slice(audio_specific_config);

    esds.push(0x06);
    write_exp_len(&mut esds, 1, 4);
    esds.push(0x02);

    esds
}

pub fn build_dops(opus_head: &[u8]) -> Vec<u8> {
    let head = if opus_head.len() > 8 && &opus_head[..8] == b"OpusHead" {
        &opus_head[8..]
    } else {
        opus_head
    };

    if head.len() < 11 {
        return vec![
            0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0xBB, 0x80, 0x00, 0x00, 0x00,
        ];
    }

    let version = 0u8;
    let channel_count = if head[1] >= 1 { head[1] } else { 2 };
    let pre_skip = u16::from_le_bytes([head[2], head[3]]);
    let sample_rate = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
    let gain = i16::from_le_bytes([head[8], head[9]]);
    let family = head[10];

    let mut dops = Vec::new();
    dops.push(version);
    dops.push(channel_count);
    dops.extend_from_slice(&pre_skip.to_be_bytes());
    dops.extend_from_slice(&sample_rate.to_be_bytes());
    dops.extend_from_slice(&gain.to_be_bytes());
    dops.push(family);

    if family != 0 && head.len() > 11 {
        dops.extend_from_slice(&head[11..]);
    }

    dops
}

/// Strip the "fLaC" signature prefix from a FLAC config, returning the
/// raw 34-byte STREAMINFO block, or None if the config is too short.
fn strip_flac_header(config: &[u8]) -> Option<&[u8]> {
    if config.len() >= 38 && &config[..4] == b"fLaC" {
        Some(&config[4..38])
    } else if config.len() >= 34 {
        Some(&config[..34])
    } else {
        None
    }
}

pub fn parse_flac_streaminfo(config: &[u8]) -> Option<(u32, u16)> {
    let si = strip_flac_header(config)?;
    let val = u64::from_be_bytes([
        si[10], si[11], si[12], si[13], si[14], si[15], si[16], si[17],
    ]);
    let sample_rate = ((val >> 44) & 0xFFFFF) as u32;
    let channel_count = (((val >> 41) & 0x7) + 1) as u16;
    Some((sample_rate, channel_count))
}

pub fn build_dfla(config: &[u8]) -> Vec<u8> {
    let mut dfla = Vec::new();
    let si = match strip_flac_header(config) {
        Some(s) => s,
        None => return dfla,
    };

    dfla.push(0x80);
    dfla.push(0x00);
    dfla.push(0x00);
    dfla.push(0x22);
    dfla.extend_from_slice(si);
    dfla
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reverse_bits_32() {
        assert_eq!(Fmp4Muxer::reverse_bits_32(0x00000001), 0x80000000);
        assert_eq!(Fmp4Muxer::reverse_bits_32(0x80000000), 0x00000001);
        assert_eq!(Fmp4Muxer::reverse_bits_32(0x60000000), 0x00000006);
        assert_eq!(Fmp4Muxer::reverse_bits_32(0xFFFFFFFF), 0xFFFFFFFF);
        assert_eq!(Fmp4Muxer::reverse_bits_32(0x00000000), 0x00000000);
    }

    #[test]
    fn test_av1c_passthrough() {
        let config = vec![0x81, 0x04, 0x0c, 0x00];
        let av1c = av1c_box_from_config(&config);
        assert_eq!(av1c, config);
    }

    #[test]
    fn test_av1c_from_raw_obu() {
        let obu = vec![0x0a, 0x04, 0x00, 0x00, 0x00, 0x40];
        let av1c = av1c_box_from_config(&obu);
        assert_eq!(av1c[0], 0x81);
        assert_eq!(av1c[1], 0x08);
        assert_eq!(av1c[2], 0x0C);
        assert_eq!(av1c[3], 0x00);
        assert_eq!(&av1c[4..], &obu[..]);
    }

    #[test]
    fn test_ensure_av1_sizeless_sequence_header_obu() {
        // Sequence Header OBU (obu_type=1) without size field: header 0x08
        let obu = vec![
            0x08, // obu_type=1, obu_extension_flag=0, obu_has_size_field=0
            0x00, 0x00, // arbitrary payload
        ];
        let result = ensure_av1_obu_size_fields(&obu);
        assert!(
            result[0] & 0x02 != 0,
            "obu_has_size_field must be set after rewrite"
        );
        assert_ne!(
            result.len(),
            obu.len(),
            "result should differ from input (size field added)"
        );
        // Parse LEB128 size
        let mut size = 0usize;
        let mut shift = 0;
        let mut pos = 1;
        loop {
            let byte = result[pos];
            size |= ((byte & 0x7F) as usize) << shift;
            pos += 1;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        assert_eq!(
            size,
            obu.len() - 1,
            "LEB128 size should equal original payload length"
        );
    }

    #[test]
    fn test_hevc_codec_string() {
        let mut muxer = super::super::Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H265, 1920, 1080);
        muxer.set_video_config(vec![
            0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0xF0,
            0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 0x03, 0x20, 0x00, 0x00, 0x03, 0x00,
            0x80, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x78, 0xAC, 0x09,
        ]);
        assert_eq!(
            muxer.video_codec_string(),
            Some("hvc1.1.6.L120".to_string())
        );
        assert_eq!(
            muxer.video_codec_string(),
            Some("hvc1.1.6.L120".to_string())
        );
    }

    #[test]
    fn test_avc_codec_string() {
        let mut muxer = super::super::Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);
        assert_eq!(muxer.video_codec_string(), Some("avc1.42C01E".to_string()));
    }

    #[test]
    fn test_av1_codec_string() {
        let mut muxer = super::super::Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::AV1, 1920, 1080);
        muxer.set_video_config(vec![0x81, 0x08, 0x0C, 0x00]);
        assert_eq!(muxer.video_codec_string(), Some("av01.0.08M8".to_string()));
    }

    #[test]
    fn test_hevc_codec_string_with_constraints() {
        let mut muxer = super::super::Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H265, 1920, 1080);
        muxer.set_video_config(vec![
            0x01, // configurationVersion
            0x01, // profile_space=0, tier_flag=0, profile_idc=1 (Main)
            0x80, 0x00, 0x00, 0x00, // profile_compatibility_flags
            0x12, 0x34, 0x00, 0x00, 0x00, 0x00, // constraint_indicator_flags
            0x78, // level_idc = 120
            0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 0x03, 0x20, 0x00, 0x00, 0x03,
            0x00, 0x80, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x78, 0xAC, 0x09,
        ]);
        assert_eq!(
            muxer.video_codec_string(),
            Some("hvc1.1.1.L120.12.34".to_string())
        );
    }
}
