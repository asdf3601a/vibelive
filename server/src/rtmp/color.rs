use super::amf::{amf_lookup, amf_next_value, amf_read_number};
use crate::hls::fmp4::{ColorConfig, HdrMetadata};

const MDCV_CHROMA_DEN: f64 = 50000.0;
const MDCV_LUMA_DEN: f64 = 10000.0;

pub fn parse_enhanced_color_config(data: &[u8]) -> ColorConfig {
    if let Some(cfg) = try_parse_amf_color_config(data) {
        return cfg;
    }
    let cp = data.first().copied().unwrap_or(1) as u16;
    let tc = data.get(1).copied().unwrap_or(1) as u16;
    let mc = data.get(2).copied().unwrap_or(1) as u16;
    let full_range = data.get(3).copied().unwrap_or(0) != 0;
    ColorConfig {
        color_primaries: cp,
        transfer_characteristics: tc,
        matrix_coefficients: mc,
        full_range,
    }
}

pub fn parse_enhanced_hdr_metadata(data: &[u8]) -> Option<HdrMetadata> {
    if let Some(hdr) = try_parse_amf_hdr_metadata(data) {
        return Some(hdr);
    }
    if !data.is_empty() && data[0] == 0x02 {
        return None;
    }
    if data.len() < 36 {
        return None;
    }
    let hdr_data = &data[4..];
    if hdr_data.len() < 4 {
        return None;
    }
    let hdr_len = u32::from_be_bytes([hdr_data[0], hdr_data[1], hdr_data[2], hdr_data[3]]) as usize;
    if hdr_len != 28 {
        return None;
    }
    let payload = hdr_data.get(4..)?;
    if payload.len() < 28 {
        return None;
    }
    let p = payload;
    let maxcll = u16::from_be_bytes([p[0], p[1]]);
    let maxfall = u16::from_be_bytes([p[2], p[3]]);
    let mdcv_data = &p[4..];
    if mdcv_data.len() < 24 {
        return None;
    }
    Some(HdrMetadata {
        max_content_light_level: maxcll,
        max_frame_average_light_level: maxfall,
        display_primaries_x: [
            u16::from_be_bytes([mdcv_data[8], mdcv_data[9]]),
            u16::from_be_bytes([mdcv_data[0], mdcv_data[1]]),
            u16::from_be_bytes([mdcv_data[4], mdcv_data[5]]),
        ],
        display_primaries_y: [
            u16::from_be_bytes([mdcv_data[10], mdcv_data[11]]),
            u16::from_be_bytes([mdcv_data[2], mdcv_data[3]]),
            u16::from_be_bytes([mdcv_data[6], mdcv_data[7]]),
        ],
        white_point_x: u16::from_be_bytes([mdcv_data[12], mdcv_data[13]]),
        white_point_y: u16::from_be_bytes([mdcv_data[14], mdcv_data[15]]),
        max_luminance: u32::from_be_bytes([
            mdcv_data[16],
            mdcv_data[17],
            mdcv_data[18],
            mdcv_data[19],
        ]),
        min_luminance: u32::from_be_bytes([
            mdcv_data[20],
            mdcv_data[21],
            mdcv_data[22],
            mdcv_data[23],
        ]),
    })
}

fn try_parse_amf_color_config(data: &[u8]) -> Option<ColorConfig> {
    let (first_marker, _, consumed) = amf_next_value(data)?;
    if first_marker != 0x02 {
        return None;
    }
    let obj = &data[consumed..];
    let (omarker, _, _) = amf_next_value(obj)?;
    if omarker != 0x03 && omarker != 0x08 {
        return None;
    }
    let source = amf_lookup(obj, "colorConfig").unwrap_or(obj);
    let read_num_field = |name: &str| -> Option<f64> {
        let v = amf_lookup(source, name)?;
        amf_read_number(v).map(|r| r.0)
    };
    let cp = read_num_field("colorPrimaries")
        .or_else(|| read_num_field("ColorPrimaries"))
        .or_else(|| read_num_field("color_primaries"))
        .unwrap_or(0.0) as u16;
    let tc = read_num_field("transferCharacteristics")
        .or_else(|| read_num_field("TransferCharacteristics"))
        .or_else(|| read_num_field("transfer_characteristics"))
        .unwrap_or(0.0) as u16;
    let mc = read_num_field("matrixCoefficients")
        .or_else(|| read_num_field("MatrixCoefficients"))
        .or_else(|| read_num_field("matrix_coefficients"))? as u16;
    let fr = read_num_field("fullRange")
        .or_else(|| read_num_field("FullRange"))
        .or_else(|| read_num_field("full_range"))
        .unwrap_or(0.0) as u8
        != 0;
    Some(ColorConfig {
        color_primaries: cp,
        transfer_characteristics: tc,
        matrix_coefficients: mc,
        full_range: fr,
    })
}

fn try_parse_amf_hdr_metadata(data: &[u8]) -> Option<HdrMetadata> {
    let (first_marker, _, consumed) = amf_next_value(data)?;
    if first_marker != 0x02 {
        return None;
    }
    let obj = &data[consumed..];
    let first_val_size = {
        let (_, _, sz) = amf_next_value(obj)?;
        sz
    };
    let cll = amf_lookup(obj, "hdrCll").or_else(|| {
        if first_val_size < obj.len() {
            amf_lookup(&obj[first_val_size..], "hdrCll")
        } else {
            None
        }
    })?;
    let mdcv = amf_lookup(obj, "hdrMdcv").or_else(|| {
        if first_val_size < obj.len() {
            amf_lookup(&obj[first_val_size..], "hdrMdcv")
        } else {
            None
        }
    })?;

    let read_num = |obj: &[u8], name: &str| -> Option<f64> {
        let v = amf_lookup(obj, name)?;
        amf_read_number(v).map(|r| r.0)
    };
    let maxcll = read_num(cll, "maxCll").or_else(|| read_num(cll, "maxCLL"))? as u16;
    let maxfall = read_num(cll, "maxFall")? as u16;

    let read_primaries = |obj: &[u8], axis: &str| -> Option<[u16; 3]> {
        let array_field = format!("displayPrimaries{}", axis);
        if let Some(v) = amf_lookup(obj, &array_field)
            && !v.is_empty()
            && v[0] == 0x0A
        {
            let mut arr = [0u16; 3];
            arr[0] = (amf_read_number(&v[5..])?.0 * MDCV_CHROMA_DEN + 0.5) as u16;
            arr[1] = (amf_read_number(&v[14..])?.0 * MDCV_CHROMA_DEN + 0.5) as u16;
            arr[2] = (amf_read_number(&v[23..])?.0 * MDCV_CHROMA_DEN + 0.5) as u16;
            return Some(arr);
        }
        let r = (read_num(obj, &format!("red{}", axis))? * MDCV_CHROMA_DEN + 0.5) as u16;
        let g = (read_num(obj, &format!("green{}", axis))? * MDCV_CHROMA_DEN + 0.5) as u16;
        let b = (read_num(obj, &format!("blue{}", axis))? * MDCV_CHROMA_DEN + 0.5) as u16;
        Some([r, g, b])
    };

    let display_primaries_x = read_primaries(mdcv, "X")?;
    let display_primaries_y = read_primaries(mdcv, "Y")?;
    let white_point_x = (read_num(mdcv, "whitePointX")? * MDCV_CHROMA_DEN + 0.5) as u16;
    let white_point_y = (read_num(mdcv, "whitePointY")? * MDCV_CHROMA_DEN + 0.5) as u16;
    let max_luminance = (read_num(mdcv, "maxLuminance")? * MDCV_LUMA_DEN + 0.5) as u32;
    let min_luminance = (read_num(mdcv, "minLuminance")? * MDCV_LUMA_DEN + 0.5) as u32;

    Some(HdrMetadata {
        max_content_light_level: maxcll,
        max_frame_average_light_level: maxfall,
        display_primaries_x,
        display_primaries_y,
        white_point_x,
        white_point_y,
        max_luminance,
        min_luminance,
    })
}
