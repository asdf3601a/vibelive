const TS_PACKET_SIZE: usize = 188;
const SYNC_BYTE: u8 = 0x47;
const PAT_PID: u16 = 0x0000;
const PMT_PID: u16 = 0x1000;
const VIDEO_PID: u16 = 0x0100;
const AUDIO_PID: u16 = 0x0101;
const PROGRAM_NUMBER: u16 = 0x0001;

#[derive(Clone, Copy, PartialEq)]
pub enum StreamType {
    H264 = 0x1B,
    H265 = 0x24,
    AV1 = 0x06,
    AAC = 0x0F,
}

impl StreamType {
    pub fn is_video(&self) -> bool {
        matches!(self, Self::H264 | Self::H265 | Self::AV1)
    }

    pub fn needs_registration_descriptor(&self) -> bool {
        matches!(self, Self::AV1)
    }

    pub fn registration_fourcc(&self) -> Option<&'static [u8; 4]> {
        match self {
            Self::AV1 => Some(b"av01"),
            _ => None,
        }
    }
}

pub struct MpegTsMuxer {
    video_codec: Option<StreamType>,
    audio_codec: Option<StreamType>,
    video_cc: u8,
    audio_cc: u8,
    has_sent_initial: bool,
    pcr_base: u64,
}

impl MpegTsMuxer {
    pub fn new() -> Self {
        Self {
            video_codec: None,
            audio_codec: None,
            video_cc: 0,
            audio_cc: 0,
            has_sent_initial: false,
            pcr_base: 0,
        }
    }

    pub fn set_video_codec(&mut self, codec: StreamType) {
        self.video_codec = Some(codec);
    }

    pub fn set_audio_codec(&mut self, codec: StreamType) {
        self.audio_codec = Some(codec);
    }

    pub fn initial_packets(&mut self) -> Vec<Vec<u8>> {
        self.has_sent_initial = true;
        vec![
            self.create_pat(),
            self.create_pmt(),
        ]
    }

    fn create_pat(&self) -> Vec<u8> {
        let mut section_data = Vec::new();
        section_data.extend_from_slice(&[0x00, 0x01]); // transport_stream_id
        section_data.push(0xC1); // version(0x60) + current_next(1)
        section_data.push(0x00); // section_number
        section_data.push(0x00); // last_section_number
        section_data.extend_from_slice(&PROGRAM_NUMBER.to_be_bytes());
        section_data.extend_from_slice(&[0xE0 | ((PMT_PID >> 8) as u8), (PMT_PID & 0xFF) as u8]);

        let section_length = (section_data.len() + 4) as u16; // +4 for CRC
        let mut psi = vec![
            0x00, // table_id
            0xB0 | ((section_length >> 8) as u8 & 0x0F),
            (section_length & 0xFF) as u8,
        ];
        psi.extend_from_slice(&section_data);
        let crc = crc32(&psi);
        psi.extend_from_slice(&crc.to_be_bytes());
        self.packetize_psi(psi, PAT_PID)
    }

    fn create_pmt(&self) -> Vec<u8> {
        let mut section_data = Vec::new();
        section_data.extend_from_slice(&PROGRAM_NUMBER.to_be_bytes());
        section_data.push(0xC1); // version(0x60) + current_next(1)
        section_data.push(0x00); // section_number
        section_data.push(0x00); // last_section_number
        section_data.extend_from_slice(&[0xE0 | ((VIDEO_PID >> 8) as u8), (VIDEO_PID & 0xFF) as u8]);

        let program_info_length: u16 = 0;
        section_data.extend_from_slice(&[0xF0 | ((program_info_length >> 8) as u8), (program_info_length & 0xFF) as u8]);

        if let Some(codec) = self.video_codec {
            section_data.push(codec as u8);
            section_data.extend_from_slice(&[0xE0 | ((VIDEO_PID >> 8) as u8), (VIDEO_PID & 0xFF) as u8]);

            if codec.needs_registration_descriptor() {
                if let Some(fcc) = codec.registration_fourcc() {
                    let es_info = vec![0x05, 4, fcc[0], fcc[1], fcc[2], fcc[3]];
                    let es_info_length = es_info.len() as u16;
                    section_data.extend_from_slice(&[0xF0 | ((es_info_length >> 8) as u8), (es_info_length & 0xFF) as u8]);
                    section_data.extend_from_slice(&es_info);
                }
            } else {
                section_data.extend_from_slice(&[0xF0, 0x00]);
            }
        }

        if let Some(codec) = self.audio_codec {
            section_data.push(codec as u8);
            section_data.extend_from_slice(&[0xE0 | ((AUDIO_PID >> 8) as u8), (AUDIO_PID & 0xFF) as u8]);
            section_data.extend_from_slice(&[0xF0, 0x00]);
        }

        let section_length = (section_data.len() + 4) as u16; // +4 for CRC
        let mut section_header = vec![
            0x02, // table_id
            0xB0 | ((section_length >> 8) as u8 & 0x0F),
            (section_length & 0xFF) as u8,
        ];
        section_header.extend_from_slice(&section_data);
        let crc = crc32(&section_header);
        section_header.extend_from_slice(&crc.to_be_bytes());
        self.packetize_psi(section_header, PMT_PID)
    }

    fn packetize_psi(&self, data: Vec<u8>, pid: u16) -> Vec<u8> {
        let mut result = Vec::new();
        let mut offset = 0;

        while offset < data.len() {
            let mut packet = [0u8; TS_PACKET_SIZE];
            packet[0] = SYNC_BYTE;
            let pusi = if offset == 0 { 1 } else { 0 };
            packet[1] = ((pusi << 6) | ((pid >> 8) as u8)) as u8;
            packet[2] = (pid & 0xFF) as u8;
            let afc: u8 = if pusi == 1 { 3 } else { 1 };
            packet[3] = (afc << 4) | 0x0F;

            let mut wp = 4;
            if pusi == 1 {
                // Add a 1-byte adaptation field (just flags=0) so that
                // payload starts at byte 6, leaving room for pointer_field.
                packet[4] = 1; // adaptation_field_length
                packet[5] = 0; // adaptation_field_flags (none set)
                wp = 6;
                // pointer_field = 0: section starts immediately after
                packet[wp] = 0;
                wp += 1;
            }

            let remaining = TS_PACKET_SIZE - wp;
            let chunk_size = remaining.min(data.len() - offset);
            packet[wp..wp + chunk_size].copy_from_slice(&data[offset..offset + chunk_size]);
            offset += chunk_size;
            result.extend_from_slice(&packet);
        }

        result
    }

    pub fn add_video_nal(&mut self, data: &[u8], dts: u64, pts: u64) -> Vec<Vec<u8>> {
        self.pcr_base = dts;

        let _ = &data;
        let _ = &pts;
        let mut packets = Vec::new();

        let pes_stream_id = match self.video_codec {
            Some(StreamType::AV1) => 0x06,
            _ => 0xE0,
        };

        let mut pes_data = Vec::new();

        if self.video_codec == Some(StreamType::H264) {
            if data.len() > 2 && data[0] == 0x17 && data[1] == 0x00 {
                let avcc_len = ((data[2] as usize) & 0x03) + 1;
                let mut nal_offset = 2 + 3 + avcc_len * 2;
                while nal_offset + 4 < data.len() {
                    let nal_size = u32::from_be_bytes([
                        data[nal_offset], data[nal_offset + 1],
                        data[nal_offset + 2], data[nal_offset + 3],
                    ]) as usize;
                    nal_offset += 4;
                    if nal_offset + nal_size <= data.len() {
                        pes_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                        pes_data.extend_from_slice(&data[nal_offset..nal_offset + nal_size]);
                        nal_offset += nal_size;
                    }
                }
            } else {
                pes_data.extend_from_slice(data);
            }
        } else {
            pes_data.extend_from_slice(data);
        }

        let pes_header = self.create_pes_header(
            dts, pts, pes_data.len() as u32 + 8, false, pes_stream_id
        );
        let mut full_data = Vec::new();
        full_data.extend_from_slice(&pes_header);
        full_data.extend_from_slice(&pes_data);

        let mut offset = 0;
        while offset < full_data.len() {
            let mut packet = [0u8; TS_PACKET_SIZE];
            packet[0] = SYNC_BYTE;
            let pusi = if offset == 0 { 1 } else { 0 };
            packet[1] = ((pusi << 6) | ((VIDEO_PID >> 8) as u8)) as u8;
            packet[2] = (VIDEO_PID & 0xFF) as u8;

            if offset == 0 {
                let adapt_len: u8 = 12;
                packet[3] = (0x01 << 6) | (3 << 4) | (self.video_cc & 0x0F);
                packet[4] = adapt_len;
                packet[5] = 0x10;
                let pcr = dts * 300;
                let pcr_base_val = pcr / 300;
                let pcr_ext = (pcr % 300) as u8;
                packet[6] = ((pcr_base_val >> 25) & 0xFF) as u8;
                packet[7] = ((pcr_base_val >> 17) & 0xFF) as u8;
                packet[8] = ((pcr_base_val >> 9) & 0xFF) as u8;
                packet[9] = ((pcr_base_val >> 1) & 0xFF) as u8;
                packet[10] = ((pcr_base_val << 7) as u8) | 0x7E | (((pcr_ext as u16 >> 8) & 0x01) as u8);
                packet[11] = pcr_ext & 0xFF;
                let payload_start = 5 + adapt_len as usize;
                for i in 12..payload_start { packet[i] = 0xFF; }
                let remaining = TS_PACKET_SIZE - payload_start;
                let chunk = if offset + remaining <= full_data.len() {
                    &full_data[offset..offset + remaining]
                } else {
                    &full_data[offset..]
                };
                packet[payload_start..payload_start + chunk.len()].copy_from_slice(chunk);
                offset += chunk.len();
            } else {
                packet[3] = (0x01 << 6) | (1 << 4) | (self.video_cc & 0x0F);
                let header_len = 4;
                let remaining = TS_PACKET_SIZE - header_len;
                let chunk = if offset + remaining <= full_data.len() {
                    &full_data[offset..offset + remaining]
                } else {
                    &full_data[offset..]
                };
                packet[header_len..header_len + chunk.len()].copy_from_slice(chunk);
                offset += chunk.len();
            }
            self.video_cc = (self.video_cc + 1) & 0x0F;
            packets.push(packet.to_vec());
        }

        packets
    }

    pub fn add_audio_aac(&mut self, data: &[u8], pts: u64) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        let pes_header = self.create_pes_header(pts, pts, data.len() as u32 + 8, true, 0xC0);
        let mut full_data = Vec::new();
        full_data.extend_from_slice(&pes_header);
        full_data.extend_from_slice(data);

        let mut offset = 0;
        while offset < full_data.len() {
            let mut packet = [0u8; TS_PACKET_SIZE];
            packet[0] = SYNC_BYTE;
            let pusi = if offset == 0 { 1 } else { 0 };
            packet[1] = ((pusi << 6) | ((AUDIO_PID >> 8) as u8)) as u8;
            packet[2] = (AUDIO_PID & 0xFF) as u8;

            if offset == 0 {
                packet[3] = (0x01 << 6) | (3 << 4) | (self.audio_cc & 0x0F);
                packet[4] = 1;
                packet[5] = 0x00;
                let payload_start = 6;
                let remaining = TS_PACKET_SIZE - payload_start;
                let chunk = if offset + remaining <= full_data.len() {
                    &full_data[offset..offset + remaining]
                } else {
                    &full_data[offset..]
                };
                packet[payload_start..payload_start + chunk.len()].copy_from_slice(chunk);
                offset += chunk.len();
            } else {
                packet[3] = (0x01 << 6) | (1 << 4) | (self.audio_cc & 0x0F);
                let remaining = TS_PACKET_SIZE - 4;
                let chunk = if offset + remaining <= full_data.len() {
                    &full_data[offset..offset + remaining]
                } else {
                    &full_data[offset..]
                };
                packet[4..4 + chunk.len()].copy_from_slice(chunk);
                offset += chunk.len();
            }
            self.audio_cc = (self.audio_cc + 1) & 0x0F;
            packets.push(packet.to_vec());
        }

        packets
    }

    fn create_pes_header(
        &self,
        _dts: u64,
        pts: u64,
        data_len: u32,
        _is_audio: bool,
        stream_id: u8,
    ) -> Vec<u8> {
        let mut header = Vec::new();
        header.push(0x00);
        header.push(0x00);
        header.push(0x01);
        header.push(stream_id);

        // For video, PES_packet_length = 0 means unspecified (valid per MPEG-2).
        // This avoids PES_packet_size mismatch when chunking across TS packets.
        let pes_len: u16 = 0;
        header.extend_from_slice(&pes_len.to_be_bytes());

        // Byte 0: '10' + scrambling(00) + priority(0) + alignment(0) + copyright(0) + original(0)
        header.push(0x80);
        // Byte 1: PTS_DTS_flags(10) + other flags all 0
        header.push(0x80);
        // Byte 2: PES_header_data_length (5 bytes for PTS only)
        header.push(5);

        let pts30 = ((pts >> 30) & 0x07) as u8;
        let pts22 = ((pts >> 22) & 0xFF) as u8;
        let pts15 = ((pts >> 15) & 0x7F) as u8;
        let pts7  = ((pts >> 7) & 0xFF) as u8;
        let pts0  = (pts & 0x7F) as u8;

        // PTS 5-byte encoding with correct marker bits
        header.push(0x21 | (pts30 << 1));
        header.push(pts22);
        header.push(0x80 | pts15);
        header.push(pts7);
        header.push(0x80 | pts0);

        header
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pat_packets() {
        let muxer = MpegTsMuxer::new();
        let pat = muxer.create_pat();
        assert!(pat.len() >= 188);
        assert_eq!(pat[0], SYNC_BYTE);

        let mut minus_sync = pat.clone();
        minus_sync.remove(0);
        assert!(!minus_sync.contains(&SYNC_BYTE));
    }

    #[test]
    fn test_pmt_with_av1() {
        let mut muxer = MpegTsMuxer::new();
        muxer.set_video_codec(StreamType::AV1);
        muxer.set_audio_codec(StreamType::AAC);
        let pmt = muxer.create_pmt();
        assert!(pmt.len() >= 188);
        assert_eq!(pmt[0], SYNC_BYTE);

        // Parse the PMT section from the TS packet
        let afc = (pmt[3] >> 4) & 0x03;
        let mut payload_offset = 4;
        if afc == 3 || afc == 2 {
            payload_offset += pmt[4] as usize + 1;
        }
        let payload = &pmt[payload_offset..];
        let ptr = payload[0] as usize;
        // Section data starts after pointer_field
        let sec = &payload[1..];
        let table_id = sec[ptr];
        assert_eq!(table_id, 0x02, "PMT table_id should be 0x02");

        let section_length = (((sec[ptr + 1] & 0x0F) as usize) << 8) | (sec[ptr + 2] as usize);
        let _section_end = ptr + 3 + section_length;

        // PCR PID at bytes ptr+8..ptr+10 of section
        let pcr_pid = (((sec[ptr + 8] & 0x1F) as u16) << 8) | (sec[ptr + 9] as u16);
        assert_eq!(pcr_pid, VIDEO_PID, "PCR PID should be video PID");

        // program_info_length
        let program_info_len = (((sec[ptr + 10] & 0x0F) as usize) << 8) | (sec[ptr + 11] as usize);
        let mut idx = ptr + 12 + program_info_len;

        // First stream descriptor (video)
        let stream_type = sec[idx];
        assert_eq!(stream_type, StreamType::AV1 as u8, "Video stream_type should be AV1 (0x06)");
        let elem_pid = (((sec[idx + 1] & 0x1F) as u16) << 8) | (sec[idx + 2] as u16);
        assert_eq!(elem_pid, VIDEO_PID);
        let es_info_len = (((sec[idx + 3] & 0x0F) as usize) << 8) | (sec[idx + 4] as usize);
        assert_eq!(es_info_len, 6, "ES_info should contain 6-byte registration descriptor");
        assert_eq!(sec[idx + 5], 0x05, "Descriptor tag should be 0x05 (registration)");
        assert_eq!(sec[idx + 6], 4, "Descriptor length should be 4");
        assert_eq!(&sec[idx + 7..idx + 11], b"av01", "Registration should be 'av01'");

        idx += 5 + es_info_len;

        // Second stream descriptor (audio)
        let audio_stream_type = sec[idx];
        assert_eq!(audio_stream_type, StreamType::AAC as u8, "Audio stream_type should be AAC (0x0F)");
        let audio_elem_pid = (((sec[idx + 1] & 0x1F) as u16) << 8) | (sec[idx + 2] as u16);
        assert_eq!(audio_elem_pid, AUDIO_PID);
    }

    #[test]
    fn test_video_packets() {
        let mut muxer = MpegTsMuxer::new();
        muxer.set_video_codec(StreamType::H264);
        let data = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84];
        let packets = muxer.add_video_nal(&data, 0, 0);
        assert!(!packets.is_empty());
        for pkt in &packets {
            assert_eq!(pkt.len(), TS_PACKET_SIZE);
            assert_eq!(pkt[0], SYNC_BYTE);
        }
    }

    #[test]
    fn test_audio_packets() {
        let mut muxer = MpegTsMuxer::new();
        let data = vec![0xAF, 0x01, 0x02, 0x03];
        let packets = muxer.add_audio_aac(&data, 0);
        assert!(!packets.is_empty());
        for pkt in &packets {
            assert_eq!(pkt.len(), TS_PACKET_SIZE);
            assert_eq!(pkt[0], SYNC_BYTE);
        }
    }

    #[test]
    fn test_stream_type_registration() {
        assert!(StreamType::AV1.needs_registration_descriptor());
        assert!(!StreamType::H264.needs_registration_descriptor());
        assert_eq!(StreamType::AV1.registration_fourcc(), Some(b"av01"));
        assert_eq!(StreamType::AV1 as u8, 0x06);
        assert_eq!(StreamType::H264 as u8, 0x1B);
    }
}