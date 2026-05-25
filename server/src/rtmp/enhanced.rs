use bytes::Bytes;

pub const ENHANCED_VIDEO_CODEC_ID: u8 = 0xCC;
pub const ENHANCED_AUDIO_CODEC_ID: u8 = 0xCA;

pub const FCC_AV1: u32 = 0x61763031; // "av01"
pub const FCC_AVC: u32 = 0x61766331; // "avc1"
pub const FCC_HEVC: u32 = 0x68766331; // "hvc1"
pub const FCC_VP09: u32 = 0x76703039; // "vp09"
pub const FCC_OPUS: u32 = 0x4F707573; // "Opus"

#[derive(Debug, Clone, PartialEq)]
pub enum VideoPacketType {
    SequenceStart = 0,
    CodedFrames = 1,
    SequenceEnd = 2,
    CodedFramesX = 3,
    Metadata = 4,
    Mpeg2TsSequenceStart = 5,
    Multitrack = 6,
    ModEx = 7,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioPacketType {
    SequenceStart = 0,
    CodedFrames = 1,
    SequenceEnd = 2,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoFrameType {
    KeyFrame = 1,
    InterFrame = 2,
    DisposableInterFrame = 3,
    GeneratedKeyFrame = 4,
    VideoInfoCmd = 5,
}

#[derive(Debug, Clone)]
pub struct EnhancedVideoHeader {
    pub packet_type: VideoPacketType,
    pub frame_type: VideoFrameType,
    pub fourcc: u32,
    pub codec: EnhancedVideoCodec,
    pub composition_time: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnhancedVideoCodec {
    Av1,
    Avc,
    Hevc,
    Vp9,
    Unknown(u32),
}

impl EnhancedVideoCodec {
    pub fn from_fourcc(fourcc: u32) -> Self {
        match fourcc {
            FCC_AV1 => Self::Av1,
            FCC_AVC => Self::Avc,
            FCC_HEVC => Self::Hevc,
            FCC_VP09 => Self::Vp9,
            _ => Self::Unknown(fourcc),
        }
    }

    pub fn mpegts_stream_type(&self) -> u8 {
        match self {
            Self::Av1 => 0x06,
            Self::Avc => 0x1B,
            Self::Hevc => 0x24,
            Self::Vp9 => 0x06,
            Self::Unknown(_) => 0x06,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnhancedAudioHeader {
    pub packet_type: AudioPacketType,
    pub fourcc: u32,
    pub codec: EnhancedAudioCodec,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnhancedAudioCodec {
    Opus,
    Unknown(u32),
}

impl EnhancedAudioCodec {
    pub fn from_fourcc(fourcc: u32) -> Self {
        match fourcc {
            FCC_OPUS => Self::Opus,
            _ => Self::Unknown(fourcc),
        }
    }
}

fn parse_video_packet_type(v: u8) -> Option<VideoPacketType> {
    match v {
        0 => Some(VideoPacketType::SequenceStart),
        1 => Some(VideoPacketType::CodedFrames),
        2 => Some(VideoPacketType::SequenceEnd),
        3 => Some(VideoPacketType::CodedFramesX),
        4 => Some(VideoPacketType::Metadata),
        5 => Some(VideoPacketType::Mpeg2TsSequenceStart),
        6 => Some(VideoPacketType::Multitrack),
        7 => Some(VideoPacketType::ModEx),
        _ => None,
    }
}

fn parse_video_frame_type(v: u8) -> Option<VideoFrameType> {
    match v {
        1 => Some(VideoFrameType::KeyFrame),
        2 => Some(VideoFrameType::InterFrame),
        3 => Some(VideoFrameType::DisposableInterFrame),
        4 => Some(VideoFrameType::GeneratedKeyFrame),
        5 => Some(VideoFrameType::VideoInfoCmd),
        _ => None,
    }
}

fn read_composition_time(data: &[u8], offset: usize) -> Result<i32, &'static str> {
    if data.len() < offset + 3 {
        return Err("data too short for composition time");
    }
    let ct_bytes: [u8; 3] = [data[offset], data[offset + 1], data[offset + 2]];
    let ct = i32::from_be_bytes([
        if ct_bytes[0] & 0x80 != 0 { 0xFF } else { 0x00 },
        ct_bytes[0], ct_bytes[1], ct_bytes[2],
    ]);
    Ok(ct)
}

/// Parse Enhanced RTMP video header.
/// Supports both the legacy 0xCC draft format and the veovera/FFmpeg format.
pub fn parse_enhanced_video_header(data: &[u8]) -> Result<(EnhancedVideoHeader, &[u8]), &'static str> {
    if data.len() < 5 {
        return Err("data too short for enhanced video header");
    }

    // Legacy 0xCC format: [0xCC, packet_type, FourCC(4), ...]
    if data[0] == ENHANCED_VIDEO_CODEC_ID && data.len() >= 6 {
        let packet_type = parse_video_packet_type(data[1])
            .ok_or("unknown video packet type")?;
        let fourcc_bytes: [u8; 4] = [data[2], data[3], data[4], data[5]];
        let fourcc = u32::from_be_bytes(fourcc_bytes);
        let codec = EnhancedVideoCodec::from_fourcc(fourcc);

        let mut consumed = 6;
        let composition_time = if codec == EnhancedVideoCodec::Avc || codec == EnhancedVideoCodec::Hevc {
            if data.len() < 9 {
                return Err("data too short for avc/hevc header");
            }
            let ct = read_composition_time(data, 6)?;
            consumed = 9;
            ct
        } else {
            0
        };

        let frame_type = if packet_type == VideoPacketType::SequenceStart {
            VideoFrameType::KeyFrame
        } else {
            VideoFrameType::InterFrame
        };

        return Ok((EnhancedVideoHeader {
            packet_type,
            frame_type,
            fourcc,
            codec,
            composition_time,
        }, &data[consumed..]));
    }

    // Veovera/FFmpeg format: [isExHeader(1) | frameType(3) | packetType(4), FourCC(4), ...]
    let first_byte = data[0];
    let is_ex_header = (first_byte & 0x80) != 0;
    if !is_ex_header {
        return Err("not an enhanced video header");
    }

    let frame_type = parse_video_frame_type(((first_byte >> 4) & 0x07) as u8)
        .ok_or("unknown video frame type")?;
    let packet_type = parse_video_packet_type((first_byte & 0x0F) as u8)
        .ok_or("unknown video packet type")?;

    let fourcc_bytes: [u8; 4] = [data[1], data[2], data[3], data[4]];
    let fourcc = u32::from_be_bytes(fourcc_bytes);
    let codec = EnhancedVideoCodec::from_fourcc(fourcc);

    let mut consumed = 5;
    let composition_time = if packet_type == VideoPacketType::CodedFrames
        && (codec == EnhancedVideoCodec::Avc || codec == EnhancedVideoCodec::Hevc) {
        let ct = read_composition_time(data, 5)?;
        consumed = 8;
        ct
    } else {
        0
    };

    Ok((EnhancedVideoHeader {
        packet_type,
        frame_type,
        fourcc,
        codec,
        composition_time,
    }, &data[consumed..]))
}

pub fn parse_enhanced_audio_header(data: &[u8]) -> Result<(EnhancedAudioHeader, &[u8]), &'static str> {
    if data.len() < 6 {
        return Err("data too short for enhanced audio header");
    }
    let packet_type_byte = data[1];
    let packet_type = match packet_type_byte {
        0 => AudioPacketType::SequenceStart,
        1 => AudioPacketType::CodedFrames,
        2 => AudioPacketType::SequenceEnd,
        _ => return Err("unknown audio packet type"),
    };

    let fourcc_bytes: [u8; 4] = [data[2], data[3], data[4], data[5]];
    let fourcc = u32::from_be_bytes(fourcc_bytes);
    let codec = EnhancedAudioCodec::from_fourcc(fourcc);

    Ok((EnhancedAudioHeader {
        packet_type,
        fourcc,
        codec,
    }, &data[6..]))
}

/// Check if data has an ExVideo FourCC at bytes 1-4 (ffmpeg's format).
fn fourcc_at_bytes_1_4(data: &[u8]) -> Option<u32> {
    if data.len() < 5 {
        return None;
    }
    let fcc = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
    match fcc {
        FCC_AV1 | FCC_AVC | FCC_HEVC | FCC_VP09 => Some(fcc),
        _ => None,
    }
}

pub fn is_enhanced_video(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    // Veovera/FFmpeg format: bit 7 set, known FourCC at bytes 1-4
    if data.len() >= 5 {
        let is_ex_header = (data[0] & 0x80) != 0;
        if is_ex_header && fourcc_at_bytes_1_4(data).is_some() {
            return true;
        }
    }
    // Legacy 0xCC format
    data.first() == Some(&ENHANCED_VIDEO_CODEC_ID)
}

pub fn is_enhanced_audio(data: &[u8]) -> bool {
    data.first() == Some(&ENHANCED_AUDIO_CODEC_ID)
}

/// Convert AVCC-format H.264 NAL units (4-byte length prefix) to Annex-B format (start code prefix).
/// Also handles AVCDecoderConfigurationRecord (SPS/PPS) which uses 2-byte length prefixes.
pub fn avcc_to_annexb(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len() + 32);

    // Detect AVCDecoderConfigurationRecord: starts with configurationVersion==1
    // and has enough bytes for the header fields.
    if data.len() >= 7 && data[0] == 1 {
        let length_size = ((data[4] as usize) & 0x03) + 1;
        if length_size == 2 || length_size == 4 {
            let num_sps = (data[5] as usize) & 0x1F;
            let mut offset = 6;
            for _ in 0..num_sps {
                if offset + 2 > data.len() { break; }
                let sps_len = ((data[offset] as usize) << 8) | (data[offset + 1] as usize);
                offset += 2;
                if offset + sps_len > data.len() { break; }
                result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                result.extend_from_slice(&data[offset..offset + sps_len]);
                offset += sps_len;
            }
            if offset < data.len() {
                let num_pps = data[offset] as usize;
                offset += 1;
                for _ in 0..num_pps {
                    if offset + 2 > data.len() { break; }
                    let pps_len = ((data[offset] as usize) << 8) | (data[offset + 1] as usize);
                    offset += 2;
                    if offset + pps_len > data.len() { break; }
                    result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                    result.extend_from_slice(&data[offset..offset + pps_len]);
                    offset += pps_len;
                }
            }
            return result;
        }
    }

    // Standard AVCC NAL unit stream: 4-byte length prefix
    let mut offset = 0;
    while offset + 4 <= data.len() {
        let nal_len = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]) as usize;
        offset += 4;
        if nal_len == 0 || offset + nal_len > data.len() {
            result.extend_from_slice(&data[offset..]);
            break;
        }
        result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        result.extend_from_slice(&data[offset..offset + nal_len]);
        offset += nal_len;
    }
    result
}

/// Strip legacy FLV audio tag header.
/// For AAC (sound format == 10), strips the 2-byte header (sound format byte + AAC packet type).
/// For other formats, strips only the 1-byte sound format byte.
pub fn strip_legacy_audio_header(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let sound_format = (data[0] & 0xF0) >> 4;
    if sound_format == 10 && data.len() >= 2 {
        // AAC: strip sound format byte + AAC packet type byte
        data[2..].to_vec()
    } else {
        data[1..].to_vec()
    }
}

pub fn parse_legacy_video_header(data: &[u8]) -> Result<(u8, u8, &[u8]), &'static str> {
    if data.is_empty() {
        return Err("empty video data");
    }
    let frame_type = (data[0] & 0xF0) >> 4;
    let codec_id = data[0] & 0x0F;
    let mut consumed = 1;

    if codec_id == 7 {
        if data.len() < 5 {
            return Err("data too short for AVC header");
        }
        let _avc_packet_type = data[1];
        consumed = 5;
        Ok((frame_type, codec_id, &data[consumed..]))
    } else {
        Ok((frame_type, codec_id, &data[consumed..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_enhanced_av1_header_legacy() {
        let data: Vec<u8> = vec![
            0xCC,             // Enhanced Video CodecID (legacy)
            1,                  // packet_type = CodedFrames
            b'a', b'v', b'0', b'1', // fourcc "av01"
            0x0A, 0x0B, 0x0C,      // AV1 OBU data
        ];
        assert!(is_enhanced_video(&data));
        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::CodedFrames);
        assert_eq!(header.codec, EnhancedVideoCodec::Av1);
        assert_eq!(header.fourcc, FCC_AV1);
        assert_eq!(remainder, &[0x0A, 0x0B, 0x0C]);
    }

    #[test]
    fn test_parse_enhanced_av1_header_veovera() {
        // Veovera/FFmpeg format: [0x80 | frame_type | packet_type, FourCC, payload]
        let data: Vec<u8> = vec![
            0x91,             // is_ex=1, frame_type=Key(1), packet_type=CodedFrames(1)
            b'a', b'v', b'0', b'1', // fourcc "av01"
            0x12, 0x00, 0x0A,      // AV1 OBU data
        ];
        assert!(is_enhanced_video(&data));
        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::CodedFrames);
        assert_eq!(header.frame_type, VideoFrameType::KeyFrame);
        assert_eq!(header.codec, EnhancedVideoCodec::Av1);
        assert_eq!(header.fourcc, FCC_AV1);
        assert_eq!(remainder, &[0x12, 0x00, 0x0A]);
    }

    #[test]
    fn test_parse_enhanced_av1_sequence_start() {
        let data: Vec<u8> = vec![
            0x90,             // is_ex=1, frame_type=Key(1), packet_type=SequenceStart(0)
            b'a', b'v', b'0', b'1', // fourcc "av01"
            0x81, 0x04, 0x0c,      // AV1 sequence header OBU
        ];
        assert!(is_enhanced_video(&data));
        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::SequenceStart);
        assert_eq!(header.frame_type, VideoFrameType::KeyFrame);
        assert_eq!(header.codec, EnhancedVideoCodec::Av1);
        assert_eq!(remainder, &[0x81, 0x04, 0x0c]);
    }

    #[test]
    fn test_parse_enhanced_avc_with_composition_time() {
        let data: Vec<u8> = vec![
            0x91,             // is_ex=1, frame_type=Key, packet_type=CodedFrames
            b'a', b'v', b'c', b'1', // fourcc "avc1"
            0x00, 0x00, 0x21, // composition time = 33ms
            0x67, 0x42, 0xC0, // NAL data
        ];
        assert!(is_enhanced_video(&data));
        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::CodedFrames);
        assert_eq!(header.composition_time, 33);
        assert_eq!(header.codec, EnhancedVideoCodec::Avc);
        assert_eq!(remainder, &[0x67, 0x42, 0xC0]);
    }

    #[test]
    fn test_parse_enhanced_inter_frame() {
        let data: Vec<u8> = vec![
            0xA1,             // is_ex=1, frame_type=Inter(2), packet_type=CodedFrames(1)
            b'a', b'v', b'0', b'1', // fourcc "av01"
            0x12, 0x00, 0x00,      // AV1 OBU data
        ];
        let (header, _) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.frame_type, VideoFrameType::InterFrame);
    }

    #[test]
    fn test_parse_legacy_avc_header() {
        let data: Vec<u8> = vec![
            0x17,             // frame_type=1 (key), codec_id=7 (AVC)
            0x00,             // AVCPacketType=0 (sequence header)
            0x00, 0x00, 0x00, // composition time
            0x01, 0x02, 0x03, // SPS/PPS data
        ];
        assert!(!is_enhanced_video(&data));
        let (frame_type, codec_id, remainder) = parse_legacy_video_header(&data).unwrap();
        assert_eq!(frame_type, 1);
        assert_eq!(codec_id, 7);
        assert_eq!(remainder, &[0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_parse_legacy_video_non_avc() {
        // codec_id=2 (Sorenson H.263) — only 1 byte consumed
        let data: Vec<u8> = vec![0x12, 0xAB, 0xCD];
        let (frame_type, codec_id, remainder) = parse_legacy_video_header(&data).unwrap();
        assert_eq!(frame_type, 1);
        assert_eq!(codec_id, 2);
        assert_eq!(remainder, &[0xAB, 0xCD]);
    }

    #[test]
    fn test_parse_legacy_video_truncated() {
        let data: Vec<u8> = vec![0x17]; // AVC but too short
        assert!(parse_legacy_video_header(&data).is_err());
    }

    #[test]
    fn test_avcc_to_annexb_single_nal() {
        // One NAL of length 4: [0x00,0x00,0x00,0x04] + [0x65,0x88,0x84,0x00]
        let avcc = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        let annexb = avcc_to_annexb(&avcc);
        assert_eq!(annexb, vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00]);
    }

    #[test]
    fn test_avcc_to_annexb_multiple_nals() {
        // NAL1 len=2: [0x67, 0xAB]
        // NAL2 len=3: [0x68, 0xCD, 0xEF]
        let avcc = vec![
            0x00, 0x00, 0x00, 0x02, 0x67, 0xAB,
            0x00, 0x00, 0x00, 0x03, 0x68, 0xCD, 0xEF,
        ];
        let annexb = avcc_to_annexb(&avcc);
        assert_eq!(annexb, vec![
            0x00, 0x00, 0x00, 0x01, 0x67, 0xAB,
            0x00, 0x00, 0x00, 0x01, 0x68, 0xCD, 0xEF,
        ]);
    }

    #[test]
    fn test_avcc_to_annexb_empty() {
        assert!(avcc_to_annexb(&[]).is_empty());
    }

    #[test]
    fn test_strip_legacy_audio_aac() {
        // sound format=10 (AAC), rate=3, size=1, type=1 => 0xAF
        // AAC packet type=1 (raw) => 0x01
        // Raw AAC data follows
        let data = vec![0xAF, 0x01, 0xAB, 0xCD];
        assert_eq!(strip_legacy_audio_header(&data), vec![0xAB, 0xCD]);
    }

    #[test]
    fn test_strip_legacy_audio_mp3() {
        // sound format=2 (MP3) => 0x2A (any second byte is data)
        let data = vec![0x2A, 0xAB, 0xCD];
        assert_eq!(strip_legacy_audio_header(&data), vec![0xAB, 0xCD]);
    }

    #[test]
    fn test_strip_legacy_audio_empty() {
        assert!(strip_legacy_audio_header(&[]).is_empty());
    }

    #[test]
    fn test_parse_enhanced_audio_header() {
        let data: Vec<u8> = vec![
            0xCA,             // Enhanced Audio CodecID
            1,                  // packet_type = CodedFrames
            b'O', b'p', b'u', b's', // fourcc "Opus"
            0x0A, 0x0B,         // Opus frame data
        ];
        assert!(is_enhanced_audio(&data));
        let (header, remainder) = parse_enhanced_audio_header(&data).unwrap();
        assert_eq!(header.packet_type, AudioPacketType::CodedFrames);
        assert_eq!(header.codec, EnhancedAudioCodec::Opus);
        assert_eq!(header.fourcc, FCC_OPUS);
        assert_eq!(remainder, &[0x0A, 0x0B]);
    }
}
