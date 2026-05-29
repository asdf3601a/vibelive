pub const ENHANCED_VIDEO_CODEC_ID: u8 = 0xCC;
pub const ENHANCED_AUDIO_CODEC_ID: u8 = 0xCA;

pub const FCC_AV1: u32 = 0x61763031; // "av01"
pub const FCC_AVC: u32 = 0x61766331; // "avc1"
pub const FCC_HEVC: u32 = 0x68766331; // "hvc1"
pub const FCC_VP09: u32 = 0x76703039; // "vp09"
pub const FCC_OPUS: u32 = 0x4F707573; // "Opus"
pub const FCC_MP4A: u32 = 0x6D703461; // "mp4a"
pub const FCC_FLAC: u32 = 0x664C6143; // "fLaC"

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
    MultichannelConfig = 4,
    Multitrack = 5,
    ModEx = 7,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MultitrackType {
    OneTrack = 0,
    ManyTracks = 1,
    ManyTracksManyCodecs = 2,
}

#[derive(Debug, Clone)]
pub struct EnhancedVideoTrack<'a> {
    pub track_id: u32,
    pub codec: EnhancedVideoCodec,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct EnhancedAudioTrack<'a> {
    pub track_id: u32,
    pub codec: EnhancedAudioCodec,
    pub payload: &'a [u8],
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
    Aac,
    Flac,
    Unknown(u32),
}

impl EnhancedAudioCodec {
    pub fn from_fourcc(fourcc: u32) -> Self {
        match fourcc {
            FCC_OPUS => Self::Opus,
            FCC_MP4A => Self::Aac,
            FCC_FLAC => Self::Flac,
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

fn parse_audio_packet_type(v: u8) -> Option<AudioPacketType> {
    match v {
        0 => Some(AudioPacketType::SequenceStart),
        1 => Some(AudioPacketType::CodedFrames),
        2 => Some(AudioPacketType::SequenceEnd),
        4 => Some(AudioPacketType::MultichannelConfig),
        5 => Some(AudioPacketType::Multitrack),
        7 => Some(AudioPacketType::ModEx),
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

        // Multitrack wrapper: only consume 0xCC + packet_type
        if packet_type == VideoPacketType::Multitrack {
            let frame_type = VideoFrameType::KeyFrame;
            return Ok((EnhancedVideoHeader {
                packet_type,
                frame_type,
                codec: EnhancedVideoCodec::Unknown(0),
            }, &data[2..]));
        }

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

    // Multitrack wrapper: only consume header + multitrack control byte
    if packet_type == VideoPacketType::Multitrack {
        return Ok((EnhancedVideoHeader {
            packet_type,
            frame_type,
            codec: EnhancedVideoCodec::Unknown(0),
        }, &data[1..]));
    }

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
    if data.is_empty() {
        return Err("data too short for enhanced audio header");
    }

    // Legacy 0xCA format: [0xCA, packet_type, FourCC(4), ...]
    if data[0] == ENHANCED_AUDIO_CODEC_ID {
        if data.len() < 6 {
            return Err("data too short for enhanced audio header (legacy)");
        }
        let packet_type = match data[1] {
            0 => AudioPacketType::SequenceStart,
            1 => AudioPacketType::CodedFrames,
            2 => AudioPacketType::SequenceEnd,
            6 => AudioPacketType::Multitrack,
            _ => return Err("unknown audio packet type (legacy)"),
        };

        if packet_type == AudioPacketType::Multitrack {
            return Ok((EnhancedAudioHeader {
                packet_type,
                codec: EnhancedAudioCodec::Unknown(0),
            }, &data[2..]));
        }

        let fourcc_bytes: [u8; 4] = [data[2], data[3], data[4], data[5]];
        let fourcc = u32::from_be_bytes(fourcc_bytes);
        let codec = EnhancedAudioCodec::from_fourcc(fourcc);

        return Ok((EnhancedAudioHeader {
            packet_type,
            codec,
        }, &data[6..]));
    }

    // Veovera/FFmpeg format: [soundFormat(4) | packetType(4), FourCC(4), ...]
    // soundFormat == 9 means extended header
    let packet_type_byte = data[0] & 0x0F;
    let packet_type = parse_audio_packet_type(packet_type_byte)
        .ok_or("unknown audio packet type")?;

    if packet_type == AudioPacketType::Multitrack {
        return Ok((EnhancedAudioHeader {
            packet_type,
            codec: EnhancedAudioCodec::Unknown(0),
        }, &data[1..]));
    }

    if data.len() < 5 {
        return Err("data too short for enhanced audio header");
    }
    let fourcc_bytes: [u8; 4] = [data[1], data[2], data[3], data[4]];
    let fourcc = u32::from_be_bytes(fourcc_bytes);
    let codec = EnhancedAudioCodec::from_fourcc(fourcc);

    Ok((EnhancedAudioHeader {
        packet_type,
        codec,
    }, &data[5..]))
}

pub fn parse_enhanced_video_multitrack(data: &[u8]) -> Result<(MultitrackType, Option<VideoPacketType>, Vec<EnhancedVideoTrack<'_>>), &'static str> {
    if data.is_empty() {
        return Err("data too short for multitrack header");
    }
    // v2 spec: UB[4] multitrack_type + UB[4] inner_packet_type in one byte
    let multitrack_type = match (data[0] >> 4) & 0x0F {
        0 => MultitrackType::OneTrack,
        1 => MultitrackType::ManyTracks,
        2 => MultitrackType::ManyTracksManyCodecs,
        _ => return Err("unknown multitrack type"),
    };
    let inner_packet_type = parse_video_packet_type(data[0] & 0x0F);

    let mut tracks = Vec::new();
    let mut offset = 1;

    // For OneTrack and ManyTracks, FourCC is shared and follows the control byte.
    // For ManyTracksManyCodecs, FourCC is per-track.
    let mut shared_codec = None;
    if multitrack_type != MultitrackType::ManyTracksManyCodecs {
        if data.len() < offset + 4 {
            return Err("data too short for shared fourcc");
        }
        let fourcc = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
        shared_codec = Some(EnhancedVideoCodec::from_fourcc(fourcc));
        offset += 4;
    }

    // OneTrack has exactly one track; ManyTracks/ManyTracksManyCodecs loop until exhausted
    loop {
        if offset >= data.len() {
            break;
        }
        if data.len() < offset + 1 {
            return Err("data too short for track id");
        }
        let track_id = data[offset] as u32;
        offset += 1;

        let codec = if multitrack_type == MultitrackType::ManyTracksManyCodecs {
            if data.len() < offset + 4 {
                return Err("data too short for manytracksmanycodecs fourcc");
            }
            let fourcc = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
            offset += 4;
            EnhancedVideoCodec::from_fourcc(fourcc)
        } else {
            shared_codec.clone().unwrap_or(EnhancedVideoCodec::Unknown(0))
        };

        let size = if multitrack_type != MultitrackType::OneTrack {
            if data.len() < offset + 3 {
                return Err("data too short for track size");
            }
            let sz = ((data[offset] as usize) << 16)
                | ((data[offset + 1] as usize) << 8)
                | (data[offset + 2] as usize);
            offset += 3;
            sz
        } else {
            data.len() - offset
        };

        if data.len() < offset + size {
            return Err("data too short for track payload");
        }
        let mut payload = &data[offset..offset + size];
        offset += size;

        // Per FFmpeg flvenc.c:1414-1415:
        // AVC/HEVC CodedFrames in multitrack have 3 leading CTS bytes
        // that must be stripped before the actual NAL data.
        if inner_packet_type == Some(VideoPacketType::CodedFrames)
            && (codec == EnhancedVideoCodec::Avc || codec == EnhancedVideoCodec::Hevc)
        {
            if payload.len() < 3 {
                return Err("data too short for avc/hevc composition time");
            }
            payload = &payload[3..];
        }

        tracks.push(EnhancedVideoTrack { track_id, codec, payload });

        if multitrack_type == MultitrackType::OneTrack {
            break;
        }
    }

    Ok((multitrack_type, inner_packet_type, tracks))
}

pub fn parse_enhanced_audio_multitrack(data: &[u8]) -> Result<(MultitrackType, Option<AudioPacketType>, Vec<EnhancedAudioTrack<'_>>), &'static str> {
    if data.is_empty() {
        return Err("data too short for multitrack header");
    }
    // v2 spec: UB[4] multitrack_type + UB[4] inner_packet_type in one byte
    let multitrack_type = match (data[0] >> 4) & 0x0F {
        0 => MultitrackType::OneTrack,
        1 => MultitrackType::ManyTracks,
        2 => MultitrackType::ManyTracksManyCodecs,
        _ => return Err("unknown multitrack type"),
    };
    let inner_packet_type = parse_audio_packet_type(data[0] & 0x0F);

    let mut tracks = Vec::new();
    let mut offset = 1;

    // For OneTrack and ManyTracks, FourCC is shared and follows the control byte.
    // For ManyTracksManyCodecs, FourCC is per-track.
    let mut shared_codec = None;
    if multitrack_type != MultitrackType::ManyTracksManyCodecs {
        if data.len() < offset + 4 {
            return Err("data too short for shared fourcc");
        }
        let fourcc = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
        shared_codec = Some(EnhancedAudioCodec::from_fourcc(fourcc));
        offset += 4;
    }

    loop {
        if offset >= data.len() {
            break;
        }
        if data.len() < offset + 1 {
            return Err("data too short for track id");
        }
        let track_id = data[offset] as u32;
        offset += 1;

        let codec = if multitrack_type == MultitrackType::ManyTracksManyCodecs {
            if data.len() < offset + 4 {
                return Err("data too short for manytracksmanycodecs fourcc");
            }
            let fourcc = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
            offset += 4;
            EnhancedAudioCodec::from_fourcc(fourcc)
        } else {
            shared_codec.clone().unwrap_or(EnhancedAudioCodec::Unknown(0))
        };

        let size = if multitrack_type != MultitrackType::OneTrack {
            if data.len() < offset + 3 {
                return Err("data too short for track size");
            }
            let sz = ((data[offset] as usize) << 16)
                | ((data[offset + 1] as usize) << 8)
                | (data[offset + 2] as usize);
            offset += 3;
            sz
        } else {
            data.len() - offset
        };

        if data.len() < offset + size {
            return Err("data too short for track payload");
        }
        let payload = &data[offset..offset + size];
        offset += size;

        tracks.push(EnhancedAudioTrack { track_id, codec, payload });

        if multitrack_type == MultitrackType::OneTrack {
            break;
        }
    }

    Ok((multitrack_type, inner_packet_type, tracks))
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
    // Veovera/FFmpeg format: bit 7 set
    if data.len() >= 5 {
        let is_ex_header = (data[0] & 0x80) != 0;
        if is_ex_header {
            // Multitrack packet: no FourCC at bytes 1-4, but still enhanced
            if data[0] & 0x0F == 6 {
                return true;
            }
            if fourcc_at_bytes_1_4(data).is_some() {
                return true;
            }
        }
    }
    // Legacy 0xCC format
    data.first() == Some(&ENHANCED_VIDEO_CODEC_ID)
}

pub fn is_enhanced_audio(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    // Veovera/FFmpeg format: soundFormat == 9 (0x90 | packet_type)
    if data[0] & 0xF0 == 0x90 {
        return true;
    }
    // Legacy 0xCA format
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
    fn test_parse_enhanced_audio_header_legacy() {
        let data: Vec<u8> = vec![
            0xCA,             // Enhanced Audio CodecID (legacy)
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

    #[test]
    fn test_parse_enhanced_audio_header_veovera() {
        let data: Vec<u8> = vec![
            0x91,             // soundFormat=9, packet_type=CodedFrames
            b'O', b'p', b'u', b's', // fourcc "Opus"
            0x0A, 0x0B,         // Opus frame data
        ];
        assert!(is_enhanced_audio(&data));
        let (header, remainder) = parse_enhanced_audio_header(&data).unwrap();
        assert_eq!(header.packet_type, AudioPacketType::CodedFrames);
        assert_eq!(header.codec, EnhancedAudioCodec::Opus);
        assert_eq!(remainder, &[0x0A, 0x0B]);
    }

    #[test]
    fn test_parse_enhanced_audio_multitrack() {
        // OBS/FFmpeg audio multitrack: [0x95, 0x01, "Opus", 0x01, payload]
        let data: Vec<u8> = vec![
            0x95,             // soundFormat=9 | Multitrack(5)
            0x01,             // OneTrack | CodedFrames(1)
            b'O', b'p', b'u', b's', // fourcc "Opus"
            0x01,             // track_id = 1
            0x0A, 0x0B,       // Opus frame data
        ];
        assert!(is_enhanced_audio(&data));
        let (header, remainder) = parse_enhanced_audio_header(&data).unwrap();
        assert_eq!(header.packet_type, AudioPacketType::Multitrack);
        let (mt, inner_pt, tracks) = parse_enhanced_audio_multitrack(remainder).unwrap();
        assert_eq!(mt, MultitrackType::OneTrack);
        assert_eq!(inner_pt, Some(AudioPacketType::CodedFrames));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].track_id, 1);
        assert_eq!(tracks[0].codec, EnhancedAudioCodec::Opus);
        assert_eq!(tracks[0].payload, &[0x0A, 0x0B]);
    }

    #[test]
    fn test_parse_enhanced_video_multitrack_onetrack() {
        // OBS/FFmpeg video multitrack: [0x96, 0x01, "av01", 0x01, payload]
        let data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame(0x10)
            0x01,             // OneTrack | CodedFrames(1)
            b'a', b'v', b'0', b'1', // fourcc "av01"
            0x01,             // track_id = 1
            0x0A, 0x0B, 0x0C, // payload
        ];
        assert!(is_enhanced_video(&data));
        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::Multitrack);
        let (mt, inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(mt, MultitrackType::OneTrack);
        assert_eq!(inner_pt, Some(VideoPacketType::CodedFrames));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].track_id, 1);
        assert_eq!(tracks[0].codec, EnhancedVideoCodec::Av1);
        assert_eq!(tracks[0].payload, &[0x0A, 0x0B, 0x0C]);
    }

    #[test]
    fn test_parse_enhanced_video_multitrack_onetrack_sequence_start() {
        let data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame(0x10)
            0x00,             // OneTrack | SequenceStart(0)
            b'a', b'v', b'0', b'1', // fourcc "av01"
            0x01,             // track_id = 1
            0x81, 0x04, 0x0c, // AV1 config payload
        ];
        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::Multitrack);
        let (mt, inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(mt, MultitrackType::OneTrack);
        assert_eq!(inner_pt, Some(VideoPacketType::SequenceStart));
        assert_eq!(tracks[0].track_id, 1);
        assert_eq!(tracks[0].codec, EnhancedVideoCodec::Av1);
    }

    #[test]
    fn test_parse_avc_multitrack_strips_cts() {
        // FFmpeg flvenc.c:1414-1415 writes 3-byte CTS after track_id for AVC/HEVC CodedFrames.
        // Data: [exVideo] [OneTrack|CodedFrames] ["avc1"] [track=1] [CTS=0x001234] [NAL data]
        let nal_data = vec![0x00, 0x00, 0x00, 0x05, 0x65, 0x88, 0x84, 0x00, 0x01];
        let mut data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame
            0x01,             // OneTrack | CodedFrames(1)
            b'a', b'v', b'c', b'1', // "avc1"
            0x00,             // track_id = 0
            0x00, 0x12, 0x34, // CTS = 0x1234 (4660)
        ];
        data.extend_from_slice(&nal_data);

        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::Multitrack);
        let (mt, inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(mt, MultitrackType::OneTrack);
        assert_eq!(inner_pt, Some(VideoPacketType::CodedFrames));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, EnhancedVideoCodec::Avc);
        // CTS bytes (0x00, 0x12, 0x34) must be stripped from payload
        assert_eq!(tracks[0].payload, nal_data.as_slice());
    }

    #[test]
    fn test_parse_avc_multitrack_codedframesx_no_cts() {
        // CodedFramesX (DTS==PTS optimization) has NO CTS bytes.
        let nal_data = vec![0x00, 0x00, 0x00, 0x05, 0x65, 0x88, 0x84, 0x00, 0x01];
        let mut data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame
            0x03,             // OneTrack | CodedFramesX(3)
            b'a', b'v', b'c', b'1', // "avc1"
            0x00,             // track_id = 0
        ];
        data.extend_from_slice(&nal_data);

        let (_header, remainder) = parse_enhanced_video_header(&data).unwrap();
        let (_mt, inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(inner_pt, Some(VideoPacketType::CodedFramesX));
        // CodedFramesX: NO CTS stripping, payload is raw NAL data
        assert_eq!(tracks[0].payload, nal_data.as_slice());
    }

    #[test]
    fn test_parse_hevc_multitrack_strips_cts() {
        let nal_data = vec![0x00, 0x00, 0x00, 0x06, 0x40, 0x01, 0x0C, 0x01, 0xFF, 0xFF];
        let mut data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame
            0x01,             // OneTrack | CodedFrames(1)
            b'h', b'v', b'c', b'1', // "hvc1"
            0x00,             // track_id = 0
            0xFF, 0xFE, 0x00, // CTS = -512 (sign-extended from 0xFFFE00)
        ];
        data.extend_from_slice(&nal_data);

        let (_header, remainder) = parse_enhanced_video_header(&data).unwrap();
        let (_mt, _inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(tracks[0].codec, EnhancedVideoCodec::Hevc);
        assert_eq!(tracks[0].payload, nal_data.as_slice());
    }

    #[test]
    fn test_parse_avc_multitrack_legacy_strips_cts() {
        // Legacy 0xCC format: [0xCC] [Multitrack|frameType] [multitrack_type] ["avc1"] [track_id] [CTS] [NAL]
        let nal_data = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88];
        let mut data: Vec<u8> = vec![
            0xCC,             // legacy ExVideo marker
            0x06,             // packet_type = Multitrack(6), frame_type=InterFrame
            0x01,             // multitrack_type: OneTrack | CodedFrames
            b'a', b'v', b'c', b'1', // "avc1"
            0x00,             // track_id = 0
            0x00, 0x00, 0x00, // CTS = 0
        ];
        data.extend_from_slice(&nal_data);

        let (header, remainder) = parse_enhanced_video_header(&data).unwrap();
        assert_eq!(header.packet_type, VideoPacketType::Multitrack);
        let (_mt, _inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(tracks[0].codec, EnhancedVideoCodec::Avc);
        assert_eq!(tracks[0].payload, nal_data.as_slice());
    }

    #[test]
    fn test_parse_avc_multitrack_sequence_start_no_cts() {
        // SequenceStart has NO CTS, only raw AVCC config.
        let config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        let mut data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame
            0x00,             // OneTrack | SequenceStart(0)
            b'a', b'v', b'c', b'1', // "avc1"
            0x00,             // track_id = 0
        ];
        data.extend_from_slice(&config);

        let (_header, remainder) = parse_enhanced_video_header(&data).unwrap();
        let (_mt, inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(inner_pt, Some(VideoPacketType::SequenceStart));
        assert_eq!(tracks[0].payload, config.as_slice());
    }

    #[test]
    fn test_parse_av1_multitrack_codedframes_no_cts_stripping() {
        // AV1 has no CTS - payload should pass through unchanged
        let obu_data = vec![0x0A, 0x04, 0x00, 0x00, 0x00, 0x40];
        let mut data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame
            0x01,             // OneTrack | CodedFrames(1)
            b'a', b'v', b'0', b'1', // "av01"
            0x00,             // track_id = 0
        ];
        data.extend_from_slice(&obu_data);

        let (_header, remainder) = parse_enhanced_video_header(&data).unwrap();
        let (_mt, _inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(tracks[0].codec, EnhancedVideoCodec::Av1);
        assert_eq!(tracks[0].payload, obu_data.as_slice());
    }

    #[test]
    fn test_parse_avc_multitrack_manytracks_strips_cts() {
        // ManyTracks with 2 AVC tracks, each with CTS
        let nal1 = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88];
        let nal2 = vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x88];
        let track0_size = 3 + nal1.len(); // CTS 3 + NAL
        let track1_size = 3 + nal2.len();
        let mut data: Vec<u8> = vec![
            0x96,             // is_ex=1 | Multitrack(6) | KeyFrame
            0x11,             // ManyTracks(1) | CodedFrames(1)
            b'a', b'v', b'c', b'1', // "avc1"
            0x00,             // track 0 id
            (track0_size >> 16) as u8, (track0_size >> 8) as u8, track0_size as u8, // size
            0x00, 0x00, 0x01, // CTS = 1
        ];
        data.extend_from_slice(&nal1);
        data.extend_from_slice(&[
            0x01,             // track 1 id
            (track1_size >> 16) as u8, (track1_size >> 8) as u8, track1_size as u8, // size
            0x00, 0x00, 0x02, // CTS = 2
        ]);
        data.extend_from_slice(&nal2);

        let (_header, remainder) = parse_enhanced_video_header(&data).unwrap();
        let (_mt, _inner_pt, tracks) = parse_enhanced_video_multitrack(remainder).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].codec, EnhancedVideoCodec::Avc);
        assert_eq!(tracks[1].codec, EnhancedVideoCodec::Avc);
        assert_eq!(tracks[0].payload, nal1.as_slice());
        assert_eq!(tracks[1].payload, nal2.as_slice());
    }
}
