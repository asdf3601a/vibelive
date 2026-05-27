// Minimal fMP4 (CMAF) writer for HLS output
// Supports H.264/H.265/AV1 video + AAC audio

use std::borrow::Cow;

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
    Flac,
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

    pub fn add_video_sample(&mut self, data: Cow<'_, [u8]>, dts: u64, pts: u64, is_keyframe: bool) {
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
            data: data.into_owned(),
            dts,
            size,
            duration: 0, // filled at flush time
            flags,
            composition_time_offset: cto,
        });
    }

    pub fn add_audio_sample(&mut self, data: Cow<'_, [u8]>, pts: u64) {
        if data.is_empty() {
            return;
        }
        if self.audio_samples.is_empty() {
            self.audio_base_pts = pts;
        }
        let size = data.len() as u32;
        self.audio_samples.push(Sample {
            data: data.into_owned(),
            dts: pts,
            size,
            duration: 0, // filled at flush time
            flags: 0,
            composition_time_offset: 0,
        });
    }

    /// Returns the duration of the last video sample, or a default if none.
    pub fn last_video_sample_duration(&self) -> u64 {
        self.video_samples.last().map(|s| s.duration as u64).unwrap_or(33)
    }

    /// Returns the duration of the last audio sample, or a default if none.
    pub fn last_audio_sample_duration(&self) -> u64 {
        self.audio_samples.last().map(|s| s.duration as u64).unwrap_or(21)
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
        let brands: &[u8] = if self.video_codec == Some(VideoCodec::AV1) {
            b"iso5mp41cmfcav01"
        } else {
            b"iso5mp41cmfv"
        };
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
        self.write_tkhd(&mut trak_data, 1, 0, (self.video_width as u32) << 16, (self.video_height as u32) << 16);
        self.write_mdia_video(&mut trak_data);
        write_box(w, b"trak", &trak_data);
    }

    fn write_audio_trak(&self, w: &mut Vec<u8>) {
        let mut trak_data = Vec::new();
        self.write_tkhd(&mut trak_data, 2, 0x0100, 0, 0);
        self.write_mdia_audio(&mut trak_data);
        write_box(w, b"trak", &trak_data);
    }

    fn write_tkhd(&self, w: &mut Vec<u8>, track_id: u32, volume: u16, width: u32, height: u32) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&track_id.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&[0u8; 8]);
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&volume.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]);
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());
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
                VideoCodec::H264 => {
                    self.write_avc1_sample_entry(&mut data);
                }
                VideoCodec::H265 => {
                    self.write_hvc1_sample_entry(&mut data);
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
            Some(AudioCodec::Flac) => {
                self.write_flac_sample_entry(&mut data);
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

    fn write_hvc1_sample_entry(&self, w: &mut Vec<u8>) {
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

        // hvcC box
        if let Some(ref config) = self.video_config {
            write_box(&mut data, b"hvcC", config);
        } else {
            write_box(&mut data, b"hvcC", &[]);
        }

        write_box(w, b"hvc1", &data);
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
        // compressorname: recommended value "\012AOM Coding" (first byte is length 10)
        let compressorname: [u8; 32] = {
            let mut buf = [0u8; 32];
            buf[0] = 10;
            buf[1..11].copy_from_slice(b"AOM Coding");
            buf
        };
        data.extend_from_slice(&compressorname);
        data.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        data.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined

        // av1C box
        if let Some(ref config) = self.video_config {
            let av1c = av1c_box_from_config(config);
            write_box(&mut data, b"av1C", &av1c);
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

    fn write_flac_sample_entry(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        data.extend_from_slice(&[0u8; 8]); // reserved

        // Parse STREAMINFO from config to get sample_rate and channels
        let (sample_rate, channel_count) = self.audio_config.as_ref()
            .and_then(|c| parse_flac_streaminfo(c))
            .unwrap_or((44100u32, 1u16));

        data.extend_from_slice(&channel_count.to_be_bytes());
        data.extend_from_slice(&16u16.to_be_bytes()); // samplesize
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&(sample_rate << 16).to_be_bytes()); // samplerate in 16.16

        // dfLa box (FullBox per FLAC-in-ISOBMFF spec)
        if let Some(ref config) = self.audio_config {
            let dfla = build_dfla(config);
            write_fullbox(&mut data, b"dfLa", 0, 0, &dfla);
        } else {
            write_fullbox(&mut data, b"dfLa", 0, 0, &[]);
        }

        write_box(w, b"fLaC", &data);
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

struct BitReader<'a> {
    data: &'a [u8],
    byte_offset: usize,
    bit_offset: u8, // 0-7, MSB first
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, byte_offset: 0, bit_offset: 0 }
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

/// Parsed fields from AV1 Sequence Header OBU needed for av1C.
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
}

/// Parse the Sequence Header OBU payload up through color_config().
fn parse_av1_sequence_header(payload: &[u8]) -> Option<Av1SeqHeader> {
    let mut r = BitReader::new(payload);
    if payload.len() < 2 {
        return None;
    }

    let seq_profile = r.read_bits(3) as u8;
    let mut h = Av1SeqHeader { seq_profile, ..Default::default() };
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
            let _ = r.read_bits(32); // num_units_in_display_tick
            let _ = r.read_bits(32); // time_scale
            let equal_picture_interval = r.read_bit();
            if equal_picture_interval == 0 {
                // Skip UVLC num_ticks_per_picture_minus_1
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
                let _ = r.read_bits(32); // num_units_in_decoding_tick
                let _ = r.read_bits(5);  // buffer_removal_time_length_minus_1
                let _ = r.read_bits(5);  // frame_presentation_time_length_minus_1
            }
        }
        initial_display_delay_present_flag = r.read_bit();
        let operating_points_cnt_minus_1 = r.read_bits(5) as u8;
        for i in 0..=operating_points_cnt_minus_1 {
            let _ = r.read_bits(12); // operating_point_idc
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
                    let _ = r.read_bits(n); // decoder_buffer_delay
                    let _ = r.read_bits(n); // encoder_buffer_delay
                    let _ = r.read_bit();   // low_delay_mode_flag
                }
            }
            if initial_display_delay_present_flag == 1 {
                let initial_display_delay_present_for_this_op = r.read_bit();
                if initial_display_delay_present_for_this_op == 1 {
                    let _ = r.read_bits(4); // initial_display_delay_minus_1
                }
            }
        }
    }

    // frame dimensions
    let frame_width_bits_minus_1 = r.read_bits(4) as u8;
    let frame_height_bits_minus_1 = r.read_bits(4) as u8;
    let n = frame_width_bits_minus_1 + 1;
    let _ = r.read_bits(n); // max_frame_width_minus_1
    let n = frame_height_bits_minus_1 + 1;
    let _ = r.read_bits(n); // max_frame_height_minus_1

    let frame_id_numbers_present_flag = if reduced_still_picture_header == 1 {
        0
    } else {
        r.read_bit()
    };
    if frame_id_numbers_present_flag == 1 {
        let _ = r.read_bits(4); // delta_frame_id_length_minus_2
        let _ = r.read_bits(3); // additional_frame_id_length_minus_1
    }

    let _ = r.read_bit(); // use_128x128_superblock
    let _ = r.read_bit(); // enable_filter_intra
    let _ = r.read_bit(); // enable_intra_edge_filter

    if reduced_still_picture_header == 0 {
        let _ = r.read_bit(); // enable_interintra_compound
        let _ = r.read_bit(); // enable_masked_compound
        let _ = r.read_bit(); // enable_warped_motion
        let _ = r.read_bit(); // enable_dual_filter
        let enable_order_hint = r.read_bit();
        if enable_order_hint == 1 {
            let _ = r.read_bit(); // enable_jnt_comp
            let _ = r.read_bit(); // enable_ref_frame_mvs
        }
        let seq_choose_screen_content_tools = r.read_bit();
        let seq_force_screen_content_tools = if seq_choose_screen_content_tools == 1 {
            2 // SELECT_SCREEN_CONTENT_TOOLS
        } else {
            r.read_bit()
        };
        if seq_force_screen_content_tools > 0 {
            let seq_choose_integer_mv = r.read_bit();
            if seq_choose_integer_mv == 0 {
                let _ = r.read_bit(); // seq_force_integer_mv
            }
        }
        if enable_order_hint == 1 {
            let _ = r.read_bits(3); // order_hint_bits_minus_1
        }
    }

    let _ = r.read_bit(); // enable_superres
    let _ = r.read_bit(); // enable_cdef
    let _ = r.read_bit(); // enable_restoration

    // color_config()
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
    let mut color_primaries = 0u8;
    let mut transfer_characteristics = 0u8;
    let mut matrix_coefficients = 0u8;
    if color_description_present_flag == 1 {
        color_primaries = r.read_bits(8) as u8;
        transfer_characteristics = r.read_bits(8) as u8;
        matrix_coefficients = r.read_bits(8) as u8;
    }

    if h.monochrome == 1 {
        let _ = r.read_bit(); // color_range
        h.chroma_subsampling_x = 1;
        h.chroma_subsampling_y = 1;
        h.chroma_sample_position = 0; // CSP_UNKNOWN
    } else if color_primaries == 1 && transfer_characteristics == 13 && matrix_coefficients == 0 {
        // CP_BT_709 == 1, TC_SRGB == 13, MC_IDENTITY == 0
        h.chroma_subsampling_x = 0;
        h.chroma_subsampling_y = 0;
        h.chroma_sample_position = 0;
    } else {
        let _ = r.read_bit(); // color_range
        if h.seq_profile == 0 {
            h.chroma_subsampling_x = 1;
            h.chroma_subsampling_y = 1;
        } else if h.seq_profile == 1 {
            h.chroma_subsampling_x = 0;
            h.chroma_subsampling_y = 0;
        } else {
            // seq_profile == 2
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
    // separate_uv_delta_q skipped (not needed)

    Some(h)
}

/// Ensure all OBUs in an AV1 config/sample stream have obu_has_size_field=1.
/// If an OBU lacks a size field and is the last OBU, its size is computed from
/// the remaining data and a LEB128 size field is inserted. OBUs in the middle
/// without size fields cannot be safely converted and cause the original data
/// to be returned unchanged.
pub(crate) fn ensure_av1_obu_size_fields(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut offset = 0;
    let mut needs_rewrite = false;

    // First pass: determine if any OBU lacks size field.
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
            }
            scan = size_start + size_bytes + size;
        } else {
            let payload_start = scan + header_size;
            if payload_start < data.len() {
                // Data follows but no size to tell us where this OBU ends.
                return data.to_vec();
            }
            needs_rewrite = true;
            break;
        }
    }

    if !needs_rewrite {
        return data.to_vec();
    }

    // Rewrite: add size field to the trailing OBU(s) without size.
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
                let byte = data[size_start + size_bytes];
                size_bytes += 1;
                size |= ((byte & 0x7F) as usize) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let obu_end = size_start + size_bytes + size;
            result.extend_from_slice(&data[offset..obu_end]);
            offset = obu_end;
        } else {
            let payload_start = offset + header_size;
            let payload_size = data.len() - payload_start;
            result.push(obu_header | 0x02); // set obu_has_size_field
            if obu_extension_flag == 1 {
                result.push(data[offset + 1]);
            }
            // LEB128 encode payload_size
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

/// Parse raw AV1 OBUs and construct a valid av1C box content.
/// If config already starts with AV1CodecConfigurationRecord (marker=1, version=1 = 0x81),
/// return it as-is. Otherwise, parse the sequence header OBU to extract profile and level.
pub fn av1c_box_from_config(config: &[u8]) -> Vec<u8> {
    if config.is_empty() {
        return vec![0x81, 0x00, 0x00, 0x00];
    }
    if config[0] == 0x81 {
        // Already an AV1CodecConfigurationRecord
        return config.to_vec();
    }

    // Parse OBU header to find sequence header
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
            offset += 1; // skip temporal_id / spatial_id
        }

        let mut obu_size = 0usize;
        if obu_has_size_field == 1 {
            let mut shift = 0;
            loop {
                if offset >= config.len() { break; }
                let byte = config[offset];
                offset += 1;
                obu_size |= ((byte & 0x7F) as usize) << shift;
                if byte & 0x80 == 0 { break; }
                shift += 7;
            }
        } else {
            // OBU extends to end of data
            obu_size = config.len().saturating_sub(offset);
        }

        if obu_type == 1 { // OBU_SEQUENCE_HEADER
            if offset + obu_size <= config.len() {
                seq_header_payload = Some(&config[offset..offset + obu_size]);
            }
            break;
        }

        offset += obu_size;
    }

    let seq = seq_header_payload.and_then(parse_av1_sequence_header);
    let h = seq.as_ref();

    let seq_profile = h.map(|s| s.seq_profile).unwrap_or(0);
    let seq_level_idx_0 = h.map(|s| s.seq_level_idx_0).unwrap_or(0);
    let seq_tier_0 = h.map(|s| s.seq_tier_0).unwrap_or(0);
    let high_bitdepth = h.map(|s| s.high_bitdepth).unwrap_or(0);
    let twelve_bit = h.map(|s| s.twelve_bit).unwrap_or(0);
    let monochrome = h.map(|s| s.monochrome).unwrap_or(0);
    let chroma_subsampling_x = h.map(|s| s.chroma_subsampling_x).unwrap_or(0);
    let chroma_subsampling_y = h.map(|s| s.chroma_subsampling_y).unwrap_or(0);
    let chroma_sample_position = h.map(|s| s.chroma_sample_position).unwrap_or(0);

    let profile_level = (seq_profile << 5) | (seq_level_idx_0 & 0x1F);
    let flags2 = (seq_tier_0 << 7)
        | (high_bitdepth << 6)
        | (twelve_bit << 5)
        | (monochrome << 4)
        | (chroma_subsampling_x << 3)
        | (chroma_subsampling_y << 2)
        | (chroma_sample_position & 0x03);
    let flags3 = 0x00; // reserved(3)=0, initial_presentation_delay_present=0, reserved(4)=0

    let mut av1c = vec![0x81, profile_level, flags2, flags3];
    // AV1-ISOBMFF requires configOBUs to have obu_has_size_field set to 1.
    let config_with_sizes = ensure_av1_obu_size_fields(config);
    av1c.extend_from_slice(&config_with_sizes);
    av1c
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

/// Parse FLAC STREAMINFO from config data.
/// Config may be either:
///   - 38 bytes: "fLaC" prefix + 34-byte STREAMINFO (standard FLV encapsulation)
///   - 34 bytes: raw STREAMINFO (Enhanced RTMP encapsulation)
/// Returns (sample_rate, channel_count).
fn parse_flac_streaminfo(config: &[u8]) -> Option<(u32, u16)> {
    let si = if config.len() >= 38 && &config[..4] == b"fLaC" {
        &config[4..38]
    } else if config.len() >= 34 {
        &config[..34]
    } else {
        return None;
    };
    // bytes 10-17 contain sample_rate(20), channels-1(3), bps-1(5), total_samples(36)
    let val = u64::from_be_bytes([
        si[10], si[11], si[12], si[13], si[14], si[15], si[16], si[17],
    ]);
    let sample_rate = ((val >> 44) & 0xFFFFF) as u32;
    let channel_count = (((val >> 41) & 0x7) + 1) as u16;
    Some((sample_rate, channel_count))
}

/// Build dfLa box content from FLAC config.
/// Config may be either:
///   - 38 bytes: "fLaC" prefix + 34-byte STREAMINFO (standard FLV encapsulation)
///   - 34 bytes: raw STREAMINFO (Enhanced RTMP encapsulation)
/// Returns a sequence of FLACMetadataBlock structures (no FullBox header).
fn build_dfla(config: &[u8]) -> Vec<u8> {
    let mut dfla = Vec::new();

    let si = if config.len() >= 38 && &config[..4] == b"fLaC" {
        &config[4..38]
    } else if config.len() >= 34 {
        &config[..34]
    } else {
        return dfla;
    };

    // STREAMINFO metadata block header: last=1, type=0, length=34
    dfla.push(0x80); // last-metadata-block-flag=1, block-type=0 (STREAMINFO)
    dfla.push(0x00);
    dfla.push(0x00);
    dfla.push(0x22); // block length = 34
    dfla.extend_from_slice(si); // STREAMINFO data

    dfla
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

        muxer.add_video_sample(vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88].into(), 0, 0, true);
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

        muxer.add_video_sample(vec![0x00, 0x00, 0x00, 0x01, 0x65].into(), 0, 0, true);
        muxer.add_audio_sample(vec![0xAF, 0x01].into(), 0);
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

    #[test]
    fn test_hevc_init_segment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H265, 1920, 1080);
        muxer.set_video_config(vec![0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 0x03, 0x20, 0x00, 0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x78, 0xAC, 0x09]);
        let init = muxer.init_segment();
        assert!(init.windows(4).any(|w| w == b"hvc1"));
        assert!(init.windows(4).any(|w| w == b"hvcC"));
    }

    #[test]
    fn test_flac_sample_entry() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Flac);
        // FLV FLAC config: "fLaC" + 34-byte STREAMINFO
        let config = vec![
            0x66, 0x4c, 0x61, 0x43, // "fLaC"
            0x12, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x24, 0x15, 0x0a, 0xc4, 0x40, 0xf0, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        assert_eq!(config.len(), 38);
        muxer.set_audio_config(config);
        let init = muxer.init_segment();
        assert!(init.windows(4).any(|w| w == b"fLaC"));
        assert!(init.windows(4).any(|w| w == b"dfLa"));

        // Verify dfLa is a FullBox with correct STREAMINFO
        let dfla_pos = init.windows(4).position(|w| w == b"dfLa").unwrap();
        let dfla_size = u32::from_be_bytes([init[dfla_pos-4], init[dfla_pos-3], init[dfla_pos-2], init[dfla_pos-1]]) as usize;
        let dfla_data = &init[dfla_pos+4..dfla_pos-4+dfla_size];
        // FullBox header: version(1) + flags(3) = 4 bytes
        assert_eq!(dfla_data[0], 0x00); // version
        assert_eq!(&dfla_data[1..4], &[0x00, 0x00, 0x00]); // flags
        // First metadata block header: last=1, type=0, length=34
        assert_eq!(dfla_data[4], 0x80);
        assert_eq!(u32::from_be_bytes([0x00, dfla_data[5], dfla_data[6], dfla_data[7]]), 34);
        // STREAMINFO data should match config[4..38]
        assert_eq!(&dfla_data[8..42], &[
            0x12, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x24, 0x15, 0x0a, 0xc4, 0x40, 0xf0, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]);
    }

    #[test]
    fn test_av1c_passthrough() {
        // Already an AV1CodecConfigurationRecord (starts with 0x81)
        let config = vec![0x81, 0x04, 0x0c, 0x00];
        let av1c = av1c_box_from_config(&config);
        assert_eq!(av1c, config);
    }

    #[test]
    fn test_av1c_from_raw_obu() {
        // Raw AV1 sequence header OBU:
        // OBU header: 0x0a = type=1 (SEQ_HDR), has_size=1
        // OBU size: 0x04 = 4 bytes
        // Sequence header payload (simplified):
        // seq_profile=0 (3 bits), still_picture=0 (1 bit), reduced_still_picture_header=0 (1 bit)
        // timing_info_present_flag=0 (1 bit), initial_display_delay_present_flag=0 (1 bit)
        // operating_points_cnt_minus_1=0 (5 bits)
        // operating_point_idc[0]=0 (12 bits)
        // seq_level_idx[0]=8 (5 bits) -> level 4.0
        // seq_tier[0]=0 (1 bit, since level > 7)
        //
        // Byte layout (MSB first):
        // Byte 0: seq_profile(3) | still(1) | reduced(1) | timing_info(1) | initial_disp(1) | op_cnt(2 MSB)
        //         = 000 | 0 | 0 | 0 | 0 | 00 = 0x00
        // Byte 1: op_cnt(3 LSB) | op_idc(4 MSB)
        //         = 000 | 0000 = 0x00
        // Byte 2: op_idc(8 LSB)
        //         = 0000_0000 = 0x00
        // Byte 3: seq_level_idx(5) | seq_tier(1) | remaining(2)
        //         = 01000 | 0 | 00 = 0x40
        //
        // So payload = [0x00, 0x00, 0x00, 0x40]
        // Full OBU = [0x0a, 0x04, 0x00, 0x00, 0x00, 0x40]
        let obu = vec![0x0a, 0x04, 0x00, 0x00, 0x00, 0x40];
        let av1c = av1c_box_from_config(&obu);
        // av1C header:
        // byte0: marker=1, version=1 -> 0x81
        // byte1: profile=0, level=8 -> 0x08
        // byte2: seq_tier=0, high_bitdepth=0, twelve_bit=0, monochrome=0,
        //        chroma_subsampling_x=1, chroma_subsampling_y=1, chroma_sample_position=0 -> 0x0C
        // byte3: reserved=0, initial_presentation_delay_present=0, reserved=0 -> 0x00
        assert_eq!(av1c[0], 0x81);
        assert_eq!(av1c[1], 0x08); // profile=0, level=8
        assert_eq!(av1c[2], 0x0C); // 4:2:0 subsampling for profile 0
        assert_eq!(av1c[3], 0x00);
        // configOBUs appended
        assert_eq!(&av1c[4..], &obu[..]);
    }
}
