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
    pub codec: EnhancedVideoCodec,
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
}

#[derive(Debug, Clone)]
pub struct EnhancedAudioHeader {
    pub packet_type: AudioPacketType,
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
        if codec == EnhancedVideoCodec::Avc || codec == EnhancedVideoCodec::Hevc {
            if data.len() < 9 {
                return Err("data too short for avc/hevc header");
            }
            consumed = 9;
        }

        let frame_type = if packet_type == VideoPacketType::SequenceStart {
            VideoFrameType::KeyFrame
        } else {
            VideoFrameType::InterFrame
        };

        return Ok((EnhancedVideoHeader {
            packet_type,
            frame_type,
            codec,
        }, &data[consumed..]));
    }

    // Veovera/FFmpeg format: [isExHeader(1) | frameType(3) | packetType(4), FourCC(4), ...]
    let first_byte = data[0];
    let is_ex_header = (first_byte & 0x80) != 0;
    if !is_ex_header {
        return Err("not an enhanced video header");
    }

    let frame_type = parse_video_frame_type((first_byte >> 4) & 0x07)
        .ok_or("unknown video frame type")?;
    let packet_type = parse_video_packet_type(first_byte & 0x0F)
        .ok_or("unknown video packet type")?;

    let fourcc_bytes: [u8; 4] = [data[1], data[2], data[3], data[4]];
    let fourcc = u32::from_be_bytes(fourcc_bytes);
    let codec = EnhancedVideoCodec::from_fourcc(fourcc);

    let mut consumed = 5;
    if packet_type == VideoPacketType::CodedFrames
        && (codec == EnhancedVideoCodec::Avc || codec == EnhancedVideoCodec::Hevc)
    {
        read_composition_time(data, 5)?;
        consumed = 8;
    }

    Ok((EnhancedVideoHeader {
        packet_type,
        frame_type,
        codec,
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
        assert_eq!(remainder, &[0x0A, 0x0B]);
    }
}
