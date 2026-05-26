// Minimal fMP4 (CMAF) writer for HLS output
// Supports H.264/H.265/AV1 video + AAC audio

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VideoCodec {
    H264,
    H265,
    AV1,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AudioCodec {
    Aac,
    Opus,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub data: Vec<u8>,
    pub dts: u64,
    pub duration: u32,
    pub size: u32,
    pub flags: u32,
    pub composition_time_offset: i32,
}

pub struct Fmp4Muxer {
    video_codec: Option<VideoCodec>,
    audio_codec: Option<AudioCodec>,
    video_width: u16,
    video_height: u16,
    video_config: Option<Vec<u8>>,
    audio_config: Option<Vec<u8>>,
    video_samples: Vec<Sample>,
    audio_samples: Vec<Sample>,
    video_sequence_number: u32,
    video_base_dts: u64,
    audio_base_pts: u64,
}

impl Fmp4Muxer {
    pub fn new() -> Self {
        Self {
            video_codec: None,
            audio_codec: None,
            video_width: 1920,
            video_height: 1080,
            video_config: None,
            audio_config: None,
            video_samples: Vec::new(),
            audio_samples: Vec::new(),
            video_sequence_number: 0,
            video_base_dts: 0,
            audio_base_pts: 0,
        }
    }

    pub fn video_codec(&self) -> Option<VideoCodec> {
        self.video_codec
    }

    pub fn set_video_codec(&mut self, codec: VideoCodec, width: u16, height: u16) {
        self.video_codec = Some(codec);
        self.video_width = width;
        self.video_height = height;
    }

    pub fn set_audio_codec(&mut self, codec: AudioCodec) {
        self.audio_codec = Some(codec);
    }

    pub fn set_video_config(&mut self, config: Vec<u8>) {
        self.video_config = Some(config);
    }

    pub fn set_audio_config(&mut self, config: Vec<u8>) {
        self.audio_config = Some(config);
    }

    pub fn init_segment(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.write_ftyp(&mut buf);
        self.write_moov(&mut buf);
        buf
    }

    pub fn add_video_sample(&mut self, data: Vec<u8>, dts: u64, pts: u64, is_keyframe: bool) {
        if self.video_samples.is_empty() {
            self.video_base_dts = dts;
        }
        let flags = if is_keyframe {
            0x02000000
        } else {
            0x01010000
        };
        let cto = (pts as i64 - dts as i64) as i32;
        let size = data.len() as u32;
        self.video_samples.push(Sample {
            data,
            dts,
            size,
            duration: 0, // filled at flush time
            flags,
            composition_time_offset: cto,
        });
    }

    pub fn add_audio_sample(&mut self, data: Vec<u8>, pts: u64) {
        if self.audio_samples.is_empty() {
            self.audio_base_pts = pts;
        }
        let size = data.len() as u32;
        self.audio_samples.push(Sample {
            data,
            dts: pts,
            size,
            duration: 0,
            flags: 0x00000000,
            composition_time_offset: 0,
        });
    }

    pub fn compute_and_set_durations(&mut self) {
        // Video durations
        if self.video_samples.len() > 1 {
            for i in 0..self.video_samples.len() - 1 {
                let dur = (self.video_samples[i + 1].dts - self.video_samples[i].dts) as u32;
                self.video_samples[i].duration = dur;
            }
            let last_dur = if self.video_samples.len() >= 2 {
                self.video_samples[self.video_samples.len() - 2].duration
            } else {
                33
            };
            if let Some(last) = self.video_samples.last_mut() {
                last.duration = last_dur;
            }
        } else if self.video_samples.len() == 1 {
            self.video_samples[0].duration = 33;
        }

        // Audio durations
        if self.audio_samples.len() > 1 {
            for i in 0..self.audio_samples.len() - 1 {
                let dur = (self.audio_samples[i + 1].dts - self.audio_samples[i].dts) as u32;
                self.audio_samples[i].duration = dur;
            }
            let last_dur = if self.audio_samples.len() >= 2 {
                self.audio_samples[self.audio_samples.len() - 2].duration
            } else {
                21
            };
            if let Some(last) = self.audio_samples.last_mut() {
                last.duration = last_dur;
            }
        } else if self.audio_samples.len() == 1 {
            self.audio_samples[0].duration = 21;
        }
    }

    #[cfg(test)]
    pub fn set_video_durations(&mut self, durations: Vec<u32>) {
        if durations.len() == self.video_samples.len() {
            for (i, dur) in durations.iter().enumerate() {
                self.video_samples[i].duration = *dur;
            }
        }
    }

    #[cfg(test)]
    pub fn set_audio_durations(&mut self, durations: Vec<u32>) {
        if durations.len() == self.audio_samples.len() {
            for (i, dur) in durations.iter().enumerate() {
                self.audio_samples[i].duration = *dur;
            }
        }
    }

    #[cfg(test)]
    pub fn flush_video_fragment(&mut self) -> Option<Vec<u8>> {
        if self.video_samples.is_empty() {
            return None;
        }
        self.compute_and_set_durations();
        self.video_sequence_number += 1;
        let mut buf = Vec::new();
        self.write_moof(&mut buf, 1, self.video_sequence_number, self.video_base_dts, &self.video_samples);
        self.write_mdat(&mut buf, &self.video_samples);
        self.video_samples.clear();
        Some(buf)
    }

    pub fn flush_combined_fragment(&mut self) -> Option<Vec<u8>> {
        if self.video_samples.is_empty() && self.audio_samples.is_empty() {
            return None;
        }
        self.compute_and_set_durations();
        self.video_sequence_number += 1;
        let mut buf = Vec::new();

        // We always write video track first if present, then audio
        let mut tracks: Vec<(u32, u64, &[Sample])> = Vec::new();
        if !self.video_samples.is_empty() {
            tracks.push((1, self.video_base_dts, &self.video_samples));
        }
        if !self.audio_samples.is_empty() {
            tracks.push((2, self.audio_base_pts, &self.audio_samples));
        }

        self.write_moof_multi(&mut buf, self.video_sequence_number, &tracks);

        // Write all sample data into single mdat
        let mut mdat_data = Vec::new();
        for (_, _, samples) in &tracks {
            for s in *samples {
                mdat_data.extend_from_slice(&s.data);
            }
        }
        let mdat_size = 8 + mdat_data.len() as u32;
        buf.extend_from_slice(&mdat_size.to_be_bytes());
        buf.extend_from_slice(b"mdat");
        buf.extend_from_slice(&mdat_data);

        self.video_samples.clear();
        self.audio_samples.clear();
        Some(buf)
    }

    // --- Box writers ---

    fn write_ftyp(&self, w: &mut Vec<u8>) {
        // ftyp box: size(4) + "ftyp"(4) + major_brand(4) + minor_version(4) + compatible_brands(...)
        let brands = b"iso5mp41cmfv";
        let size = 8 + 4 + 4 + brands.len();
        w.extend_from_slice(&(size as u32).to_be_bytes());
        w.extend_from_slice(b"ftyp");
        w.extend_from_slice(b"iso5");
        w.extend_from_slice(&0x00000200u32.to_be_bytes());
        w.extend_from_slice(brands);
    }

    fn write_moov(&self, w: &mut Vec<u8>) {
        let mut moov_data = Vec::new();
        self.write_mvhd(&mut moov_data);
        if self.video_codec.is_some() {
            self.write_video_trak(&mut moov_data);
        }
        if self.audio_codec.is_some() {
            self.write_audio_trak(&mut moov_data);
        }
        self.write_mvex(&mut moov_data);
        write_box(w, b"moov", &moov_data);
    }

    fn write_mvhd(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        data.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        data.extend_from_slice(&1000u32.to_be_bytes()); // timescale
        data.extend_from_slice(&0u32.to_be_bytes()); // duration
        data.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate
        data.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
        data.extend_from_slice(&[0u8; 10]); // reserved
        data.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]); // matrix
        data.extend_from_slice(&[0u8; 24]); // pre_defined
        let next_track_id: u32 = if self.audio_codec.is_some() { 3 } else { 2 };
        data.extend_from_slice(&next_track_id.to_be_bytes());
        write_fullbox(w, b"mvhd", 0, 0, &data);
    }

    fn write_video_trak(&self, w: &mut Vec<u8>) {
        let mut trak_data = Vec::new();
        self.write_tkhd_video(&mut trak_data);
        self.write_mdia_video(&mut trak_data);
        write_box(w, b"trak", &trak_data);
    }

    fn write_audio_trak(&self, w: &mut Vec<u8>) {
        let mut trak_data = Vec::new();
        self.write_tkhd_audio(&mut trak_data);
        self.write_mdia_audio(&mut trak_data);
        write_box(w, b"trak", &trak_data);
    }

    fn write_tkhd_video(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        data.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        data.extend_from_slice(&1u32.to_be_bytes()); // track_id
        data.extend_from_slice(&0u32.to_be_bytes()); // reserved
        data.extend_from_slice(&0u32.to_be_bytes()); // duration
        data.extend_from_slice(&[0u8; 8]); // reserved
        data.extend_from_slice(&0u16.to_be_bytes()); // layer
        data.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
        data.extend_from_slice(&0u16.to_be_bytes()); // volume (for video)
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]); // matrix
        data.extend_from_slice(&((self.video_width as u32) << 16).to_be_bytes()); // width
        data.extend_from_slice(&((self.video_height as u32) << 16).to_be_bytes()); // height
        write_fullbox(w, b"tkhd", 0, 0x000003, &data);
    }

    fn write_tkhd_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&2u32.to_be_bytes()); // track_id
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes()); // duration
        data.extend_from_slice(&[0u8; 8]);
        data.extend_from_slice(&0u16.to_be_bytes()); // layer
        data.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
        data.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]); // matrix
        data.extend_from_slice(&0u32.to_be_bytes()); // width
        data.extend_from_slice(&0u32.to_be_bytes()); // height
        write_fullbox(w, b"tkhd", 0, 0x000003, &data);
    }

    fn write_mdia_video(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_mdhd(&mut data, 1000);
        self.write_hdlr(&mut data, b"vide", b"VideoHandler\0");
        self.write_minf_video(&mut data);
        write_box(w, b"mdia", &data);
    }

    fn write_mdia_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_mdhd(&mut data, 1000);
        self.write_hdlr(&mut data, b"soun", b"SoundHandler\0");
        self.write_minf_audio(&mut data);
        write_box(w, b"mdia", &data);
    }

    fn write_mdhd(&self, w: &mut Vec<u8>, timescale: u32) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        data.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        data.extend_from_slice(&timescale.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes()); // duration
        data.extend_from_slice(&0x55C4u16.to_be_bytes()); // language 'und' packed
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        write_fullbox(w, b"mdhd", 0, 0, &data);
    }

    fn write_hdlr(&self, w: &mut Vec<u8>, handler_type: &[u8; 4], name: &[u8]) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
        data.extend_from_slice(handler_type);
        data.extend_from_slice(&[0u8; 12]); // reserved
        data.extend_from_slice(name);
        write_fullbox(w, b"hdlr", 0, 0, &data);
    }

    fn write_minf_video(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_vmhd(&mut data);
        self.write_dinf(&mut data);
        self.write_stbl_video(&mut data);
        write_box(w, b"minf", &data);
    }

    fn write_minf_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_smhd(&mut data);
        self.write_dinf(&mut data);
        self.write_stbl_audio(&mut data);
        write_box(w, b"minf", &data);
    }

    fn write_vmhd(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_be_bytes()); // graphicsmode
        data.extend_from_slice(&[0u8; 6]); // opcolor
        write_fullbox(w, b"vmhd", 0, 0x000001, &data);
    }

    fn write_smhd(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_be_bytes()); // balance
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        write_fullbox(w, b"smhd", 0, 0, &data);
    }

    fn write_dinf(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        let mut dref_data = Vec::new();
        dref_data.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        let url_data = Vec::new();
        write_fullbox(&mut dref_data, b"url ", 0, 0x000001, &url_data);
        write_fullbox(&mut data, b"dref", 0, 0, &dref_data);
        write_box(w, b"dinf", &data);
    }

    fn write_stbl_video(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_stsd_video(&mut data);
        self.write_empty_stbl_box(&mut data, b"stts");
        self.write_empty_stbl_box(&mut data, b"stsc");
        self.write_stsz(&mut data);
        self.write_empty_stbl_box(&mut data, b"stco");
        write_box(w, b"stbl", &data);
    }

    fn write_stbl_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_stsd_audio(&mut data);
        self.write_empty_stbl_box(&mut data, b"stts");
        self.write_empty_stbl_box(&mut data, b"stsc");
        self.write_stsz(&mut data);
        self.write_empty_stbl_box(&mut data, b"stco");
        write_box(w, b"stbl", &data);
    }

    fn write_empty_stbl_box(&self, w: &mut Vec<u8>, box_type: &[u8; 4]) {
        let data = 0u32.to_be_bytes(); // entry_count = 0
        write_fullbox(w, box_type, 0, 0, &data);
    }

    fn write_stsz(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0
        data.extend_from_slice(&0u32.to_be_bytes()); // sample_count = 0
        write_fullbox(w, b"stsz", 0, 0, &data);
    }

    fn write_stsd_video(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        if let Some(codec) = self.video_codec {
            match codec {
                VideoCodec::H264 | VideoCodec::H265 => {
                    self.write_avc1_sample_entry(&mut data);
                }
                VideoCodec::AV1 => {
                    self.write_av01_sample_entry(&mut data);
                }
            }
        }
        write_fullbox(w, b"stsd", 0, 0, &data);
    }

    fn write_stsd_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        match self.audio_codec {
            Some(AudioCodec::Opus) => {
                self.write_opus_sample_entry(&mut data);
            }
            _ => {
                self.write_mp4a_sample_entry(&mut data);
            }
        }
        write_fullbox(w, b"stsd", 0, 0, &data);
    }

    fn write_avc1_sample_entry(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&[0u8; 12]); // pre_defined
        data.extend_from_slice(&self.video_width.to_be_bytes());
        data.extend_from_slice(&self.video_height.to_be_bytes());
        data.extend_from_slice(&0x00480000u32.to_be_bytes()); // horizresolution
        data.extend_from_slice(&0x00480000u32.to_be_bytes()); // vertresolution
        data.extend_from_slice(&0u32.to_be_bytes()); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // frame_count
        data.extend_from_slice(&[0u8; 32]); // compressorname
        data.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        data.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined

        // avcC box
        if let Some(ref config) = self.video_config {
            write_box(&mut data, b"avcC", config);
        } else {
            write_box(&mut data, b"avcC", &[]);
        }

        write_box(w, b"avc1", &data);
    }

    fn write_av01_sample_entry(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&[0u8; 12]); // pre_defined
        data.extend_from_slice(&self.video_width.to_be_bytes());
        data.extend_from_slice(&self.video_height.to_be_bytes());
        data.extend_from_slice(&0x00480000u32.to_be_bytes()); // horizresolution
        data.extend_from_slice(&0x00480000u32.to_be_bytes()); // vertresolution
        data.extend_from_slice(&0u32.to_be_bytes()); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // frame_count
        data.extend_from_slice(&[0u8; 32]); // compressorname
        data.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        data.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined

        // av1C box
        if let Some(ref config) = self.video_config {
            write_box(&mut data, b"av1C", config);
        } else {
            // Minimal av1C with version=1, marker=1
            write_box(&mut data, b"av1C", &[0x81, 0x00, 0x00, 0x00]);
        }

        write_box(w, b"av01", &data);
    }

    fn write_mp4a_sample_entry(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        data.extend_from_slice(&[0u8; 8]); // reserved
        data.extend_from_slice(&0u16.to_be_bytes()); // channelcount (will be overridden by esds)
        data.extend_from_slice(&0u16.to_be_bytes()); // samplesize
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&0u32.to_be_bytes()); // samplerate (16.16 fixed, overridden by esds)

        // esds box (fullbox)
        if let Some(ref config) = self.audio_config {
            let esds_data = build_esds(config);
            write_fullbox(&mut data, b"esds", 0, 0, &esds_data);
        } else {
            write_fullbox(&mut data, b"esds", 0, 0, &[]);
        }

        write_box(w, b"mp4a", &data);
    }

    fn write_opus_sample_entry(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        data.extend_from_slice(&[0u8; 8]); // reserved
        data.extend_from_slice(&2u16.to_be_bytes()); // channelcount
        data.extend_from_slice(&16u16.to_be_bytes()); // samplesize
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&(48000u32 << 16).to_be_bytes()); // samplerate 48000 in 16.16

        // dOps box: convert OpusHead (version=1, little-endian) to dOps (version=0, big-endian)
        if let Some(ref config) = self.audio_config {
            let dops = build_dops(config);
            write_box(&mut data, b"dOps", &dops);
        } else {
            // Minimal dOps: version=0, channel_count=2, pre_skip=0, sample_rate=48000, gain=0, mapping_family=0
            write_box(&mut data, b"dOps", &[0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0xBB, 0x80, 0x00, 0x00, 0x00]);
        }

        write_box(w, b"Opus", &data);
    }

    fn write_mvex(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        if self.video_codec.is_some() {
            self.write_trex(&mut data, 1);
        }
        if self.audio_codec.is_some() {
            self.write_trex(&mut data, 2);
        }
        write_box(w, b"mvex", &data);
    }

    fn write_trex(&self, w: &mut Vec<u8>, track_id: u32) {
        let mut data = Vec::new();
        data.extend_from_slice(&track_id.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes()); // default_sample_description_index
        data.extend_from_slice(&0u32.to_be_bytes()); // default_sample_duration
        data.extend_from_slice(&0u32.to_be_bytes()); // default_sample_size
        data.extend_from_slice(&0u32.to_be_bytes()); // default_sample_flags
        write_fullbox(w, b"trex", 0, 0, &data);
    }

    // --- Fragment writers ---

    #[cfg(test)]
    fn write_moof(&self, w: &mut Vec<u8>, track_id: u32, sequence_number: u32, base_dts: u64, samples: &[Sample]) {
        let mut moof_data = Vec::new();
        self.write_mfhd(&mut moof_data, sequence_number);
        let data_offset = self.compute_single_track_data_offset(samples);
        self.write_traf(&mut moof_data, track_id, base_dts, samples, data_offset);
        write_box(w, b"moof", &moof_data);
    }

    fn write_moof_multi(&self, w: &mut Vec<u8>, sequence_number: u32, tracks: &[(u32, u64, &[Sample])]) {
        let mut moof_data = Vec::new();
        self.write_mfhd(&mut moof_data, sequence_number);

        // Compute total moof size first
        let mut moof_size = 8 + 16; // moof header + mfhd
        for (track_id, base_dts, samples) in tracks {
            let _ = (track_id, base_dts);
            moof_size += self.compute_traf_size(samples);
        }

        let mdat_header_size = 8;
        let mut current_data_offset = moof_size + mdat_header_size;

        for (track_id, base_dts, samples) in tracks {
            let track_data_offset = current_data_offset;
            self.write_traf(&mut moof_data, *track_id, *base_dts, samples, track_data_offset as u32);
            current_data_offset += samples.iter().map(|s| s.data.len()).sum::<usize>();
        }

        write_box(w, b"moof", &moof_data);
    }

    #[cfg(test)]
    fn compute_single_track_data_offset(&self, samples: &[Sample]) -> u32 {
        let traf_size = self.compute_traf_size(samples);
        let moof_size = 8 + 16 + traf_size; // moof header + mfhd + traf
        (moof_size + 8) as u32 // +8 for mdat header
    }

    fn compute_traf_size(&self, samples: &[Sample]) -> usize {
        let has_cto = samples.iter().any(|s| s.composition_time_offset != 0);
        let trun_sample_size = 12 + if has_cto { 4 } else { 0 };
        let trun_size = 12 + 8 + samples.len() * trun_sample_size;
        8 + 16 + 20 + trun_size // traf header + tfhd + tfdt + trun
    }

    fn write_mfhd(&self, w: &mut Vec<u8>, sequence_number: u32) {
        let data = sequence_number.to_be_bytes();
        write_fullbox(w, b"mfhd", 0, 0, &data);
    }

    fn write_traf(&self, w: &mut Vec<u8>, track_id: u32, base_dts: u64, samples: &[Sample], data_offset: u32) {
        let mut traf_data = Vec::new();
        self.write_tfhd(&mut traf_data, track_id);
        self.write_tfdt(&mut traf_data, base_dts);
        self.write_trun(&mut traf_data, samples, data_offset);
        write_box(w, b"traf", &traf_data);
    }

    fn write_tfhd(&self, w: &mut Vec<u8>, track_id: u32) {
        // flags = default-base-is-moof (0x020000)
        let mut data = Vec::new();
        data.extend_from_slice(&track_id.to_be_bytes());
        write_fullbox(w, b"tfhd", 0, 0x020000, &data);
    }

    fn write_tfdt(&self, w: &mut Vec<u8>, base_dts: u64) {
        let data = base_dts.to_be_bytes();
        write_fullbox(w, b"tfdt", 1, 0, &data);
    }

    fn write_trun(&self, w: &mut Vec<u8>, samples: &[Sample], data_offset: u32) {
        let has_cto = samples.iter().any(|s| s.composition_time_offset != 0);
        let mut flags: u32 = 0x000001 | 0x000100 | 0x000200 | 0x000400; // data_offset, duration, size, flags
        if has_cto {
            flags |= 0x000800;
        }

        let mut data = Vec::new();
        data.extend_from_slice(&(samples.len() as u32).to_be_bytes()); // sample_count
        data.extend_from_slice(&data_offset.to_be_bytes()); // data_offset

        for s in samples {
            data.extend_from_slice(&s.duration.to_be_bytes());
            data.extend_from_slice(&s.size.to_be_bytes());
            data.extend_from_slice(&s.flags.to_be_bytes());
            if has_cto {
                data.extend_from_slice(&s.composition_time_offset.to_be_bytes());
            }
        }

        write_fullbox(w, b"trun", 0, flags, &data);
    }

    #[cfg(test)]
    fn write_mdat(&self, w: &mut Vec<u8>, samples: &[Sample]) {
        let mut data = Vec::new();
        for s in samples {
            data.extend_from_slice(&s.data);
        }
        let size = 8 + data.len() as u32;
        w.extend_from_slice(&size.to_be_bytes());
        w.extend_from_slice(b"mdat");
        w.extend_from_slice(&data);
    }
}

fn write_box(w: &mut Vec<u8>, box_type: &[u8; 4], data: &[u8]) {
    let size = (8 + data.len()) as u32;
    w.extend_from_slice(&size.to_be_bytes());
    w.extend_from_slice(box_type);
    w.extend_from_slice(data);
}

fn write_fullbox(w: &mut Vec<u8>, box_type: &[u8; 4], version: u8, flags: u32, data: &[u8]) {
    let size = (12 + data.len()) as u32;
    w.extend_from_slice(&size.to_be_bytes());
    w.extend_from_slice(box_type);
    w.push(version);
    w.extend_from_slice(&flags.to_be_bytes()[1..]);
    w.extend_from_slice(data);
}

fn build_esds(audio_specific_config: &[u8]) -> Vec<u8> {
    let mut esds = Vec::new();

    // DecoderSpecificInfo (tag=0x05) length
    let dsi_len = audio_specific_config.len();

    // DecoderConfigDescriptor (tag=0x04) data length:
    // objectTypeIndication(1) + streamType(1) + bufferSizeDB(3) + maxBitrate(4) + avgBitrate(4)
    // + DecoderSpecificInfo tag(1) + dsi_len_length(1) + dsi_len
    let dcd_data_len = 1 + 1 + 3 + 4 + 4 + 1 + 1 + dsi_len;

    // ES_Descriptor (tag=0x03) data length:
    // ES_ID(2) + flags(1) + DecoderConfigDescriptor tag(1) + dcd_data_len_length(1) + dcd_data_len
    // + SLConfigDescriptor tag(1) + sl_len_length(1) + sl_data(1)
    let es_data_len = 2 + 1 + 1 + 1 + dcd_data_len + 1 + 1 + 1;

    // ES_Descriptor tag=0x03
    esds.push(0x03);
    write_descriptor_length(&mut esds, es_data_len);
    esds.extend_from_slice(&1u16.to_be_bytes()); // ES_ID
    esds.push(0x00); // streamDependenceFlag=0, URL_Flag=0, OCRstreamFlag=0, streamPriority=0

    // DecoderConfigDescriptor tag=0x04
    esds.push(0x04);
    write_descriptor_length(&mut esds, dcd_data_len);
    esds.push(0x40); // objectTypeIndication = MPEG-4 AAC
    esds.push(0x15); // streamType=5 (audio), upStream=0, reserved=1
    esds.extend_from_slice(&[0u8; 3]); // bufferSizeDB
    esds.extend_from_slice(&0u32.to_be_bytes()); // maxBitrate
    esds.extend_from_slice(&0u32.to_be_bytes()); // avgBitrate

    // DecoderSpecificInfo tag=0x05
    esds.push(0x05);
    write_descriptor_length(&mut esds, dsi_len);
    esds.extend_from_slice(audio_specific_config);

    // SLConfigDescriptor tag=0x06
    esds.push(0x06);
    esds.push(0x01); // length
    esds.push(0x02); // predefined = 2

    esds
}

/// Convert OpusHead (RFC 7845) to dOps box content (ISOBMFF).
/// OpusHead: version(1), channel_count(1), pre_skip(2 LE), sample_rate(4 LE), gain(2 LE), family(1)
/// dOps:     version(0), channel_count(1), pre_skip(2 BE), sample_rate(4 BE), gain(2 BE), family(1)
fn build_dops(opus_head: &[u8]) -> Vec<u8> {
    // Strip "OpusHead" signature if present
    let head = if opus_head.len() > 8 && &opus_head[..8] == b"OpusHead" {
        &opus_head[8..]
    } else {
        opus_head
    };

    if head.len() < 11 {
        // Return minimal dOps
        return vec![0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0xBB, 0x80, 0x00, 0x00, 0x00];
    }

    let version = 0u8;
    let channel_count = head[1];
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

    // Channel mapping bytes if family != 0
    if family != 0 && head.len() > 10 {
        dops.extend_from_slice(&head[10..]);
    }

    dops
}

fn write_descriptor_length(w: &mut Vec<u8>, mut len: usize) {
    // MPEG-4 descriptor length: if >= 128, use multi-byte
    if len < 128 {
        w.push(len as u8);
    } else {
        let mut bytes = Vec::new();
        while len > 0 {
            bytes.push((len & 0x7F) as u8 | 0x80);
            len >>= 7;
        }
        bytes.reverse();
        // Clear high bit on last byte
        if let Some(last) = bytes.last_mut() {
            *last &= 0x7F;
        }
        w.extend_from_slice(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ftyp_box() {
        let muxer = Fmp4Muxer::new();
        let init = muxer.init_segment();
        assert!(!init.is_empty());
        assert_eq!(&init[4..8], b"ftyp");
        assert_eq!(&init[8..12], b"iso5");
    }

    #[test]
    fn test_moov_structure() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00]);
        let init = muxer.init_segment();
        // Should contain ftyp + moov
        assert!(init.windows(4).any(|w| w == b"moov"));
        assert!(init.windows(4).any(|w| w == b"mvhd"));
        assert!(init.windows(4).any(|w| w == b"trak"));
        assert!(init.windows(4).any(|w| w == b"mvex"));
    }

    #[test]
    fn test_video_fragment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);

        muxer.add_video_sample(vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88], 0, 0, true);
        muxer.set_video_durations(vec![33]);

        let frag = muxer.flush_video_fragment();
        assert!(frag.is_some());
        let frag = frag.unwrap();
        assert!(frag.windows(4).any(|w| w == b"moof"));
        assert!(frag.windows(4).any(|w| w == b"mdat"));
    }

    #[test]
    fn test_combined_fragment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);
        muxer.set_audio_config(vec![0x12, 0x10]);

        muxer.add_video_sample(vec![0x00, 0x00, 0x00, 0x01, 0x65], 0, 0, true);
        muxer.add_audio_sample(vec![0xAF, 0x01], 0);
        muxer.set_video_durations(vec![33]);
        muxer.set_audio_durations(vec![21]);

        let frag = muxer.flush_combined_fragment().unwrap();
        assert!(frag.len() > 16);
        assert!(frag.windows(4).any(|w| w == b"moof"));
        assert!(frag.windows(4).any(|w| w == b"mdat"));
    }

    #[test]
    fn test_moov_video_only() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1280, 720);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);
        let init = muxer.init_segment();
        // Should contain exactly one trak (video only)
        assert_eq!(init.windows(4).filter(|w| *w == b"trak").count(), 1);
        assert!(init.windows(4).any(|w| w == b"avc1"));
    }
}
