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
    audio_base_dts: u64,
    audio_sample_rate: u32,
    video_timescale: u32,
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
            audio_base_dts: 0,
            audio_sample_rate: 44100,
            video_timescale: 90000,
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
        // Extract sample rate from AAC audio config
        if let Some(codec) = self.audio_codec {
            match codec {
                AudioCodec::Aac => {
                    if config.len() >= 2 {
                        // AudioSpecificConfig: byte0[7..3]=AOT, byte0[2..0]|byte1[7]=sample_rate_index, byte1[6..3]=channel_config
                        // sample_rate_index = (byte0[2..0] << 1) | byte1[7]
                        const AAC_SAMPLE_RATES: [u32; 16] = [
                            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050,
                            16000, 12000, 11025, 8000, 7350, 0, 0, 0,
                        ];
                        let rate_index = ((config[0] as u32 & 0x07) << 1) | ((config[1] as u32 >> 7) & 0x01);
                        if (rate_index as usize) < AAC_SAMPLE_RATES.len() {
                            self.audio_sample_rate = AAC_SAMPLE_RATES[rate_index as usize];
                        }
                    }
                }
                AudioCodec::Opus => {
                    // Opus always uses 48000Hz
                    self.audio_sample_rate = 48000;
                }
                AudioCodec::Flac => {
                    // Parse STREAMINFO for sample rate; default to 44100
                    if let Some((sr, _)) = parse_flac_streaminfo(&config) {
                        self.audio_sample_rate = sr;
                    }
                }
            }
        }
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
            self.audio_base_dts = pts;
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

    pub fn last_video_sample_duration(&self) -> u64 {
        self.video_samples.last().map(|s| s.duration as u64).unwrap_or(33)
    }

    pub fn last_audio_sample_duration(&self) -> u64 {
        self.audio_samples.last().map(|s| s.duration as u64).unwrap_or(21)
    }

    pub fn compute_and_set_durations(&mut self) {
        if !self.video_samples.is_empty() {
            let avg_dur = if self.video_samples.len() > 1 {
                let total = self.video_samples.last().unwrap().dts - self.video_samples.first().unwrap().dts;
                (total / (self.video_samples.len() - 1) as u64) as u32
            } else {
                33
            };
            for s in &mut self.video_samples {
                s.duration = avg_dur;
            }
        }

        if !self.audio_samples.is_empty() {
            let avg_dur = if self.audio_samples.len() > 1 {
                let total = self.audio_samples.last().unwrap().dts - self.audio_samples.first().unwrap().dts;
                (total / (self.audio_samples.len() - 1) as u64) as u32
            } else {
                21
            };
            for s in &mut self.audio_samples {
                s.duration = avg_dur;
            }
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

        let mut moof_buf = Vec::new();
        self.write_moof(&mut moof_buf, 1, self.video_sequence_number, self.video_base_dts, &self.video_samples);

        let mdat_payload_size: usize = self.video_samples.iter().map(|s| s.data.len()).sum();
        let mdat_size = 8 + mdat_payload_size;

        let mut buf = Vec::new();

        buf.extend_from_slice(&moof_buf);

        buf.extend_from_slice(&((mdat_size as u32).to_be_bytes()));
        buf.extend_from_slice(b"mdat");
        for s in &self.video_samples {
            buf.extend_from_slice(&s.data);
        }

        self.video_samples.clear();
        Some(buf)
    }

    pub fn flush_combined_fragment(&mut self) -> Option<Vec<u8>> {
        if self.video_samples.is_empty() && self.audio_samples.is_empty() {
            return None;
        }
        self.compute_and_set_durations();
        self.video_sequence_number += 1;

        // Build track list: video first, then audio
        let mut tracks: Vec<(u32, u64, &[Sample])> = Vec::new();
        if !self.video_samples.is_empty() {
            tracks.push((1, self.video_base_dts, &self.video_samples));
        }
        if !self.audio_samples.is_empty() {
            tracks.push((2, self.audio_base_dts, &self.audio_samples));
        }

        // Build moof
        let mut moof_buf = Vec::new();
        self.write_moof_multi(&mut moof_buf, self.video_sequence_number, &tracks);

        // Compute mdat size
        let mdat_payload_size: usize = tracks.iter()
            .map(|(_, _, samples)| samples.iter().map(|s| s.data.len()).sum::<usize>())
            .sum();
        let mdat_size = 8 + mdat_payload_size;

        let mut buf = Vec::new();

        buf.extend_from_slice(&moof_buf);

        // mdat
        buf.extend_from_slice(&((mdat_size as u32).to_be_bytes()));
        buf.extend_from_slice(b"mdat");
        for (_, _, samples) in &tracks {
            for s in *samples {
                buf.extend_from_slice(&s.data);
            }
        }

        self.video_samples.clear();
        self.audio_samples.clear();
        Some(buf)
    }

    // --- Box writers ---

    fn write_ftyp(&self, w: &mut Vec<u8>) {
        // ftyp box: size(4) + "ftyp"(4) + major_brand(4) + minor_version(4) + compatible_brands(...)
        let mut brands = Vec::new();
        brands.extend_from_slice(b"iso6");
        brands.extend_from_slice(b"mp41");
        brands.extend_from_slice(b"cmfc");
        if self.video_codec.is_some() {
            brands.extend_from_slice(b"cmfv");
        }
        if self.audio_codec.is_some() {
            brands.extend_from_slice(b"cmfa");
        }
        if self.video_codec == Some(VideoCodec::AV1) {
            brands.extend_from_slice(b"av01");
        }
        let size = 8 + 4 + 4 + brands.len();
        w.extend_from_slice(&(size as u32).to_be_bytes());
        w.extend_from_slice(b"ftyp");
        // CMAF major brand per ISO/IEC 23000-19
        w.extend_from_slice(b"cmfc");
        w.extend_from_slice(&0x00000200u32.to_be_bytes());
        w.extend_from_slice(&brands);
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
        // udta: encoder metadata
        let mut udta_data = Vec::new();
        let mut meta_data = Vec::new();
        let mut hdlr_data = Vec::new();
        hdlr_data.extend_from_slice(&0u32.to_be_bytes());
        hdlr_data.extend_from_slice(b"mdir");
        hdlr_data.extend_from_slice(&[0u8; 12]);
        hdlr_data.extend_from_slice(b"appl\0");
        write_fullbox(&mut meta_data, b"hdlr", 0, 0, &hdlr_data);
        // ilst with encoding tool info
        let mut ilst_data = Vec::new();
        let mut too_data = Vec::new();
        let tool_name = b"LivestreamServer";
        let mut data_data = Vec::new();
        data_data.extend_from_slice(&0u32.to_be_bytes()); // locale = 0
        data_data.extend_from_slice(tool_name);
        write_fullbox(&mut too_data, b"data", 0, 0x000001, &data_data);
        write_box(&mut ilst_data, b"\xA9too", &too_data);
        write_box(&mut meta_data, b"ilst", &ilst_data);
        write_fullbox(&mut udta_data, b"meta", 0, 0, &meta_data);
        write_box(&mut moov_data, b"udta", &udta_data);

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
        data.extend_from_slice(&2u32.to_be_bytes()); // next_track_id = 2 (required by Chrome MSE)
        write_fullbox(w, b"mvhd", 0, 0, &data);
    }

    fn write_video_trak(&self, w: &mut Vec<u8>) {
        let mut trak_data = Vec::new();
        self.write_tkhd(&mut trak_data, 1, 0, (self.video_width as u32) << 16, (self.video_height as u32) << 16);
        self.write_edts(&mut trak_data, true);
        self.write_mdia_video(&mut trak_data);
        write_box(w, b"trak", &trak_data);
    }

    fn write_audio_trak(&self, w: &mut Vec<u8>) {
        let mut trak_data = Vec::new();
        self.write_tkhd(&mut trak_data, 2, 0x0100, 0, 0);
        self.write_edts(&mut trak_data, false);
        self.write_mdia_audio(&mut trak_data);
        write_box(w, b"trak", &trak_data);
    }

    fn write_edts(&self, w: &mut Vec<u8>, is_video: bool) {
        let mut elst_data = Vec::new();
        if is_video {
            // 2 entries: empty edit + normal edit (matches ffmpeg CMAF output)
            elst_data.extend_from_slice(&2u32.to_be_bytes()); // entry_count = 2
            // Entry 1: empty edit (seg_dur=0, media_time=-1)
            elst_data.extend_from_slice(&0u32.to_be_bytes()); // segment_duration
            elst_data.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes()); // media_time = -1
            elst_data.extend_from_slice(&1u16.to_be_bytes()); // media_rate_integer
            elst_data.extend_from_slice(&0u16.to_be_bytes()); // media_rate_fraction
            // Entry 2: normal playback
            elst_data.extend_from_slice(&0u32.to_be_bytes()); // segment_duration
            elst_data.extend_from_slice(&0u32.to_be_bytes()); // media_time = 0
            elst_data.extend_from_slice(&1u16.to_be_bytes()); // media_rate_integer
            elst_data.extend_from_slice(&0u16.to_be_bytes()); // media_rate_fraction
        } else {
            elst_data.extend_from_slice(&1u32.to_be_bytes()); // entry_count = 1
            elst_data.extend_from_slice(&0u32.to_be_bytes()); // segment_duration
            elst_data.extend_from_slice(&0u32.to_be_bytes()); // media_time = 0
            elst_data.extend_from_slice(&1u16.to_be_bytes()); // media_rate_integer
            elst_data.extend_from_slice(&0u16.to_be_bytes()); // media_rate_fraction
        }
        let mut edts_data = Vec::new();
        write_fullbox(&mut edts_data, b"elst", 0, 0, &elst_data);
        write_box(w, b"edts", &edts_data);
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
        self.write_mdhd(&mut data, self.video_timescale);
        self.write_hdlr(&mut data, b"vide", b"VideoHandler\0");
        self.write_minf_video(&mut data);
        write_box(w, b"mdia", &data);
    }

    fn write_mdia_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        // Use actual audio sample rate as mdhd timescale (from RTMP config)
        self.write_mdhd(&mut data, self.audio_sample_rate);
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
        // compressorname
        let cn = b"LivestreamServer\0";
        let mut compressorname = [0u8; 32];
        compressorname[0] = cn.len() as u8;
        compressorname[1..=cn.len()].copy_from_slice(cn);
        data.extend_from_slice(&compressorname);
        data.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        data.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined

        // avcC box
        if let Some(ref config) = self.video_config {
            write_box(&mut data, b"avcC", config);
        } else {
            write_box(&mut data, b"avcC", &[]);
        }

        // Pixel aspect ratio 1:1 (inside sample entry, required by demuxers)
        let mut pasp_data = Vec::new();
        pasp_data.extend_from_slice(&1u32.to_be_bytes()); // hSpacing
        pasp_data.extend_from_slice(&1u32.to_be_bytes()); // vSpacing
        write_box(&mut data, b"pasp", &pasp_data);

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
        // compressorname
        let cn = b"LivestreamServer\0";
        let mut compressorname = [0u8; 32];
        compressorname[0] = cn.len() as u8;
        compressorname[1..=cn.len()].copy_from_slice(cn);
        data.extend_from_slice(&compressorname);
        data.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        data.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined

        // hvcC box
        if let Some(ref config) = self.video_config {
            write_box(&mut data, b"hvcC", config);
        } else {
            write_box(&mut data, b"hvcC", &[]);
        }

        let mut pasp_data = Vec::new();
        pasp_data.extend_from_slice(&1u32.to_be_bytes());
        pasp_data.extend_from_slice(&1u32.to_be_bytes());
        write_box(&mut data, b"pasp", &pasp_data);

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

        // Pixel aspect ratio 1:1
        let mut pasp_data = Vec::new();
        pasp_data.extend_from_slice(&1u32.to_be_bytes());
        pasp_data.extend_from_slice(&1u32.to_be_bytes());
        write_box(&mut data, b"pasp", &pasp_data);

        write_box(w, b"av01", &data);
    }

    fn write_mp4a_sample_entry(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        data.extend_from_slice(&[0u8; 8]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // channelcount = 1 (mono)
        data.extend_from_slice(&16u16.to_be_bytes()); // samplesize = 16
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        // samplerate in 16.16 fixed point: self.audio_sample_rate << 16
        data.extend_from_slice(&(self.audio_sample_rate << 16).to_be_bytes());

        // esds box (fullbox) with known-good AAC esds
        if let Some(ref config) = self.audio_config {
            // Use dynamic builder for non-AAC codecs
            if self.audio_codec != Some(AudioCodec::Aac) {
                let esds_data = build_esds(config);
                write_fullbox(&mut data, b"esds", 0, 0, &esds_data);
            } else {
                // For AAC, use exact bytes matching ffmpeg (proven to work with Chrome MSE)
                let mut esds_data = Vec::new();
                // ES_Descriptor: tag(03) + len(4) + ES_ID(2) + flags(1)
                esds_data.push(0x03);
                esds_data.extend_from_slice(&[0x80, 0x80, 0x80, 0x25]); // len=37
                esds_data.extend_from_slice(&1u16.to_be_bytes()); // ES_ID
                esds_data.push(0x00); // flags
                // DecoderConfig: tag(04) + len(4) + objType(1) + streamType(1) + buffer(3)
                esds_data.push(0x04);
                esds_data.extend_from_slice(&[0x80, 0x80, 0x80, 0x17]); // len=23
                esds_data.push(0x40); // MPEG-4 AAC
                esds_data.push(0x15); // AudioStream
                esds_data.extend_from_slice(&[0u8; 3]); // bufferSizeDB
                // maxBitrate + avgBitrate
                esds_data.extend_from_slice(&128000u32.to_be_bytes());
                esds_data.extend_from_slice(&128000u32.to_be_bytes());
                // DecoderSpecificInfo: tag(05) + len(4) + config data
                esds_data.push(0x05);
                esds_data.extend_from_slice(&[0x80, 0x80, 0x80, 0x05]); // len=5
                esds_data.extend_from_slice(config);
                // SLConfig: tag(06) + len(4) + data
                esds_data.push(0x06);
                esds_data.extend_from_slice(&[0x80, 0x80, 0x80, 0x01]); // len=1
                esds_data.push(0x02); // predefined
                write_fullbox(&mut data, b"esds", 0, 0, &esds_data);
            }
        } else {
            write_fullbox(&mut data, b"esds", 0, 0, &[]);
        }

        // btrt box (bitrate info)
        let mut btrt_data = Vec::new();
        btrt_data.extend_from_slice(&0u32.to_be_bytes()); // bufferSizeDB
        btrt_data.extend_from_slice(&128000u32.to_be_bytes()); // maxBitrate
        btrt_data.extend_from_slice(&128000u32.to_be_bytes()); // avgBitrate
        write_box(&mut data, b"btrt", &btrt_data);

        write_box(w, b"mp4a", &data);
    }

    fn write_opus_sample_entry(&self, w: &mut Vec<u8>) {
        // Extract actual channel count from OpusHead config so it matches dOps.
        let channel_count: u8 = self.audio_config.as_ref()
            .and_then(|c| {
                let head = if c.len() > 8 && &c[..8] == b"OpusHead" { &c[8..] } else { c.as_slice() };
                // Need at least 11 bytes for a valid OpusHead
                if head.len() >= 11 { Some(head[1]) } else { None }
            })
            // Validate: channel count must be 1..=255 (Chrome MSE rejects 0)
            .filter(|&cc| cc >= 1)
            .unwrap_or(2);

        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]); // reserved
        data.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        data.extend_from_slice(&[0u8; 8]); // reserved
        data.extend_from_slice(&(channel_count as u16).to_be_bytes()); // channelcount
        data.extend_from_slice(&16u16.to_be_bytes()); // samplesize
        data.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        data.extend_from_slice(&0u16.to_be_bytes()); // reserved
        data.extend_from_slice(&(48000u32 << 16).to_be_bytes()); // samplerate 48000 in 16.16

        // dOps box: convert OpusHead (version=1, little-endian) to dOps (version=0, big-endian)
        if let Some(ref config) = self.audio_config {
            let dops = build_dops(config);
            write_box(&mut data, b"dOps", &dops);
        } else {
            // Minimal dOps with matching channel_count
            write_box(&mut data, b"dOps", &[
                0x00, channel_count, 0x00, 0x00, 0x00, 0x00, 0xBB, 0x80, 0x00, 0x00, 0x00,
            ]);
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
        let data_offset = self.compute_single_track_data_offset(track_id, samples);
        self.write_traf(&mut moof_data, track_id, base_dts, samples, data_offset);
        write_box(w, b"moof", &moof_data);
    }

    fn write_moof_multi(&self, w: &mut Vec<u8>, sequence_number: u32, tracks: &[(u32, u64, &[Sample])]) {
        let mut moof_data = Vec::new();
        self.write_mfhd(&mut moof_data, sequence_number);

        // Compute total moof size first
        let mut moof_size = 8 + 16; // moof header + mfhd
        for (track_id, _base_dts, samples) in tracks {
            moof_size += self.compute_traf_size(*track_id, samples);
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
    fn compute_single_track_data_offset(&self, track_id: u32, samples: &[Sample]) -> u32 {
        let traf_size = self.compute_traf_size(track_id, samples);
        let moof_size = 8 + 16 + traf_size; // moof header + mfhd + traf
        (moof_size + 8) as u32 // +8 for mdat header
    }

    fn compute_traf_size(&self, track_id: u32, samples: &[Sample]) -> usize {
        let is_video = track_id == 1;
        let has_cto = samples.iter().any(|s| s.composition_time_offset != 0);

        let tfhd_size = 28; // 12 + 4(track_id) + 4(duration) + 4(size) + 4(flags)

        // trun header: 12(fullbox) + 4(sample_count) + 4(data_offset) [+ 4(first_sample_flags) for video]
        let mut trun_header = 12 + 4 + 4;
        if is_video {
            trun_header += 4; // first_sample_flags
        }

        // entry size: size(4) [+ cto(4) if has_cto]
        // Audio uses tfhd default for duration (no per-sample duration)
        let mut entry_size = 4; // size
        if has_cto {
            entry_size += 4; // cto
        }

        let trun_size = trun_header + samples.len() * entry_size;
        8 + tfhd_size + 20 + trun_size // traf header + tfhd + tfdt + trun
    }

    fn write_mfhd(&self, w: &mut Vec<u8>, sequence_number: u32) {
        let data = sequence_number.to_be_bytes();
        write_fullbox(w, b"mfhd", 0, 0, &data);
    }

    fn write_traf(&self, w: &mut Vec<u8>, track_id: u32, base_dts: u64, samples: &[Sample], data_offset: u32) {
        let is_video = track_id == 1;
        let (duration_scale, ts_scale) = if is_video {
            // Scale video durations from RTMP ms to video mdhd timescale
            (self.video_timescale as u64, 1000u64)
        } else {
            // Scale audio durations from RTMP ms to audio sample rate timescale
            (self.audio_sample_rate as u64, 1000u64)
        };
        let scaled_duration = samples.first().map(|s| s.duration as u64 * duration_scale / ts_scale).unwrap_or(if is_video { 33 } else { 21 }) as u32;
        let scaled_base_dts = base_dts * duration_scale / ts_scale;
        let default_size = samples.first().map(|s| s.size).unwrap_or(0);
        let default_flags = if is_video { 0x01010000 } else { 0x02000000 };

        let mut traf_data = Vec::new();
        self.write_tfhd(&mut traf_data, track_id, scaled_duration, default_size, default_flags);
        self.write_tfdt(&mut traf_data, scaled_base_dts);
        self.write_trun(&mut traf_data, track_id, samples, data_offset);
        write_box(w, b"traf", &traf_data);
    }

    fn write_tfhd(&self, w: &mut Vec<u8>, track_id: u32, default_duration: u32, default_size: u32, default_flags: u32) {
        // flags = default-base-is-moof (0x020000) + default-sample-duration (0x000008)
        //         + default-sample-size (0x000010) + default-sample-flags (0x000020)
        let flags = 0x020000 | 0x000008 | 0x000010 | 0x000020;
        let mut data = Vec::new();
        data.extend_from_slice(&track_id.to_be_bytes());
        data.extend_from_slice(&default_duration.to_be_bytes());
        data.extend_from_slice(&default_size.to_be_bytes());
        data.extend_from_slice(&default_flags.to_be_bytes());
        write_fullbox(w, b"tfhd", 0, flags, &data);
    }

    fn write_tfdt(&self, w: &mut Vec<u8>, base_dts: u64) {
        let data = base_dts.to_be_bytes();
        write_fullbox(w, b"tfdt", 1, 0, &data);
    }

    fn write_trun(&self, w: &mut Vec<u8>, track_id: u32, samples: &[Sample], data_offset: u32) {
        let is_video = track_id == 1;
        let has_cto = samples.iter().any(|s| s.composition_time_offset != 0);

        // Video trun: data_offset + first_sample_flags + sample_size (+ optional CTO)
        // Audio trun: data_offset + sample_size only (duration from tfhd default)
        let mut flags: u32 = 0x000001; // data_offset-present
        if is_video {
            flags |= 0x000004; // first-sample-flags-present
        }
        flags |= 0x000200; // sample-size-present
        if has_cto {
            flags |= 0x000800; // sample-composition-time-offset-present
        }

        let mut data = Vec::new();
        data.extend_from_slice(&(samples.len() as u32).to_be_bytes()); // sample_count
        data.extend_from_slice(&data_offset.to_be_bytes()); // data_offset

        if is_video {
            let first_flags = samples.first().map(|s| s.flags).unwrap_or(0x02000000);
            data.extend_from_slice(&first_flags.to_be_bytes()); // first_sample_flags
        }

        for s in samples {
            data.extend_from_slice(&s.size.to_be_bytes());
            if has_cto {
                data.extend_from_slice(&s.composition_time_offset.to_be_bytes());
            }
        }

        write_fullbox(w, b"trun", 0, flags, &data);
    }

    // --- Codec string generation (RFC 6381) ---

    pub fn codec_string(&self) -> Option<String> {
        let v = self.video_codec_string()?;
        let a = self.audio_codec_string()?;
        Some(format!("{},{}", v, a))
    }

    fn video_codec_string(&self) -> Option<String> {
        let codec = self.video_codec?;
        match codec {
            VideoCodec::H264 => {
                let config = self.video_config.as_ref()?;
                if config.len() < 4 {
                    return None;
                }
                let profile = config[1];
                let compat = config[2];
                let level = config[3];
                Some(format!("avc1.{:02X}{:02X}{:02X}", profile, compat, level))
            }
            VideoCodec::H265 => {
                let config = self.video_config.as_ref()?;
                if config.len() < 13 {
                    return None;
                }
                let byte1 = config[1];
                let profile_space = (byte1 >> 6) & 0x03;
                let tier_flag = (byte1 >> 5) & 0x01;
                let profile_idc = byte1 & 0x1F;
                let profile_compatibility = u32::from_be_bytes([config[2], config[3], config[4], config[5]]);
                let constraints = &config[6..12];
                let level_idc = config[12];
                let tier = if tier_flag == 1 { 'H' } else { 'L' };
                let constraints_hex = constraints.iter().map(|b| format!("{:02X}", b)).collect::<String>();
                Some(format!(
                    "hvc1.{}.{}.{:08X}.{}.{}{}",
                    profile_space,
                    profile_idc,
                    profile_compatibility,
                    tier,
                    level_idc,
                    if constraints_hex.chars().all(|c| c == '0') {
                        String::new()
                    } else {
                        format!(".{}", constraints_hex.trim_start_matches('0'))
                    }
                ))
            }
            VideoCodec::AV1 => {
                let config = self.video_config.as_ref()?;
                let av1c = if config[0] == 0x81 {
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
                Some(format!("av01.{}.{:02}{}{}", profile, level_idx, tier, bit_depth))
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
                if shift > 56 {
                    // Malformed LEB128
                    return data.to_vec();
                }
            }
            let new_scan = size_start.saturating_add(size_bytes).saturating_add(size);
            if new_scan > data.len() || new_scan <= scan {
                return data.to_vec();
            }
            scan = new_scan;
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


// Calculate the byte size of a descriptor length field for a given value
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

    // Write expanded descriptor length (big-endian, continuation-bit encoded)
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

    // === ES_Descriptor (tag=0x03) ===
    esds.push(0x03);
    // es_data_len = ES_ID(2) + flags(1) + DCD_tag(1) + DCD_len(4) + DCD_body + SL_tag(1) + SL_len(4) + SL_data(1)
    let dcd_body_len = 1 + 1 + 3 + 4 + 4 + 1 + 4 + dsi_len; // obj+stream+buffer+max+avg+DSI_tag+DSI_len(4)+DSI_data
    let es_data_len = 2 + 1 + 1 + 4 + dcd_body_len + 1 + 4 + 1;
    write_exp_len(&mut esds, es_data_len, 4); // 4 bytes for value < 2^28
    esds.extend_from_slice(&1u16.to_be_bytes()); // ES_ID
    esds.push(0x00); // flags

    // === DecoderConfigDescriptor (tag=0x04) ===
    esds.push(0x04);
    write_exp_len(&mut esds, dcd_body_len, 4); // 4 bytes
    esds.push(0x40); // objectTypeIndication = MPEG-4 AAC
    esds.push(0x15); // streamType=5 (audio)
    esds.extend_from_slice(&[0u8; 3]); // bufferSizeDB
    esds.extend_from_slice(&128000u32.to_be_bytes()); // maxBitrate
    esds.extend_from_slice(&128000u32.to_be_bytes()); // avgBitrate

    // === DecoderSpecificInfo (tag=0x05) ===
    esds.push(0x05);
    write_exp_len(&mut esds, dsi_len, 4); // 4 bytes (matches ffmpeg)
    esds.extend_from_slice(audio_specific_config);

    // === SLConfigDescriptor (tag=0x06) ===
    esds.push(0x06);
    write_exp_len(&mut esds, 1, 4); // 4 bytes (matches ffmpeg)
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

    // Channel mapping bytes if family != 0
    if family != 0 && head.len() > 10 {
        dops.extend_from_slice(&head[10..]);
    }

    dops
}

/// Parse FLAC STREAMINFO from config data.
///
/// Config may be either:
///
///   - 38 bytes: "fLaC" prefix + 34-byte STREAMINFO (standard FLV encapsulation)
///   - 34 bytes: raw STREAMINFO (Enhanced RTMP encapsulation)
///
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
///
/// Config may be either:
///
///   - 38 bytes: "fLaC" prefix + 34-byte STREAMINFO (standard FLV encapsulation)
///   - 34 bytes: raw STREAMINFO (Enhanced RTMP encapsulation)
///
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
        // CMAF major brand per ISO/IEC 23000-19
        assert_eq!(&init[8..12], b"cmfc");
        // compatible_brands should include iso6 (fMP4 support)
        assert!(init.windows(4).any(|w| w == b"iso6"));
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
        // Segment structure: moof + mdat
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
        // Segment structure: moof + mdat
        assert!(frag.windows(4).any(|w| w == b"moof"));
        assert!(frag.windows(4).any(|w| w == b"mdat"));
    }

    /// Parse a box at offset, returning (box_type, box_size, data_offset, data_len)
    fn parse_box(data: &[u8], offset: usize) -> Option<(&[u8], usize, usize, usize)> {
        if offset + 8 > data.len() {
            return None;
        }
        let size = u32::from_be_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]) as usize;
        let box_type = &data[offset+4..offset+8];
        let data_offset = offset + 8;
        let data_len = size.saturating_sub(8);
        Some((box_type, size, data_offset, data_len))
    }

    #[test]
    fn test_combined_fragment_large_segment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);
        muxer.set_audio_config(vec![0x12, 0x10]);

        let video_data = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00];
        let audio_data = vec![0xAF, 0x01];

        // Simulate 2s at 30fps + 48kHz AAC (~1024 samples/frame = ~93 frames)
        for i in 0..60 {
            muxer.add_video_sample(video_data.clone().into(), i as u64 * 33, i as u64 * 33, i % 30 == 0);
        }
        for i in 0..93 {
            muxer.add_audio_sample(audio_data.clone().into(), i as u64 * 21);
        }

        let frag = muxer.flush_combined_fragment().unwrap();
        println!("Large fragment size: {}", frag.len());

        let mut offset = 0;
        let mut boxes = Vec::new();
        while offset < frag.len() {
            let (box_type, size, data_offset, data_len) = parse_box(&frag, offset).unwrap();
            println!("Box {} at offset {} size {}", std::str::from_utf8(box_type).unwrap(), offset, size);
            boxes.push((box_type.to_vec(), offset, size, data_offset, data_len));
            offset += size;
        }

        // Segment structure: moof + mdat
        assert_eq!(boxes.len(), 2, "Expected moof+mdat, got {} boxes", boxes.len());
        assert_eq!(&boxes[0].0, b"moof");
        assert_eq!(&boxes[1].0, b"mdat");

        let moof_offset = boxes[0].1;
        let moof_size = boxes[0].2;
        let moof_data_offset = boxes[0].3;
        let moof_end = moof_offset + moof_size;
        let mut moof_off = moof_data_offset;
        let mut moof_boxes = Vec::new();
        while moof_off < moof_end {
            let (box_type, size, data_offset, data_len) = parse_box(&frag, moof_off).unwrap();
            moof_boxes.push((box_type.to_vec(), moof_off, size, data_offset, data_len));
            moof_off += size;
        }

        assert_eq!(moof_boxes.len(), 3);
        assert_eq!(&moof_boxes[0].0, b"mfhd");
        assert_eq!(&moof_boxes[1].0, b"traf");
        assert_eq!(&moof_boxes[2].0, b"traf");

        // Verify video trun
        let video_traf_data = moof_boxes[1].3;
        let video_traf_end = moof_boxes[1].1 + moof_boxes[1].2;
        let mut traf_off = video_traf_data;
        while traf_off < video_traf_end {
            let (box_type, size, data_offset, _) = parse_box(&frag, traf_off).unwrap();
            if box_type == b"trun" {
                let sample_count = u32::from_be_bytes([frag[data_offset+4], frag[data_offset+5], frag[data_offset+6], frag[data_offset+7]]);
                println!("Video trun sample_count = {}", sample_count);
                assert_eq!(sample_count, 60);
                break;
            }
            traf_off += size;
        }

        // Verify audio trun
        let audio_traf_data = moof_boxes[2].3;
        let audio_traf_end = moof_boxes[2].1 + moof_boxes[2].2;
        let mut traf_off = audio_traf_data;
        while traf_off < audio_traf_end {
            let (box_type, size, data_offset, _) = parse_box(&frag, traf_off).unwrap();
            if box_type == b"trun" {
                let sample_count = u32::from_be_bytes([frag[data_offset+4], frag[data_offset+5], frag[data_offset+6], frag[data_offset+7]]);
                println!("Audio trun sample_count = {}", sample_count);
                assert_eq!(sample_count, 93);
                break;
            }
            traf_off += size;
        }
    }

    #[test]
    fn test_combined_fragment_box_structure() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);
        muxer.set_audio_config(vec![0x12, 0x10]);

        // 1 keyframe video sample + 1 audio sample
        let video_data = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00];
        let audio_data = vec![0xAF, 0x01];
        muxer.add_video_sample(video_data.clone().into(), 0, 0, true);
        muxer.add_audio_sample(audio_data.clone().into(), 0);
        muxer.set_video_durations(vec![33]);
        muxer.set_audio_durations(vec![21]);

        let frag = muxer.flush_combined_fragment().unwrap();
        println!("Fragment size: {}", frag.len());

        // Parse top-level boxes
        let mut offset = 0;
        let mut boxes = Vec::new();
        while offset < frag.len() {
            let (box_type, size, data_offset, data_len) = parse_box(&frag, offset).unwrap();
            println!("Box {} at offset {} size {}", std::str::from_utf8(box_type).unwrap(), offset, size);
            boxes.push((box_type.to_vec(), offset, size, data_offset, data_len));
            offset += size;
        }

        // Segment structure: moof + mdat
        assert_eq!(boxes.len(), 2, "Expected moof+mdat, got {} boxes", boxes.len());
        assert_eq!(&boxes[0].0, b"moof");
        assert_eq!(&boxes[1].0, b"mdat");

        // Parse moof
        let moof_offset = boxes[0].1;
        let moof_size = boxes[0].2;
        let moof_data_offset = boxes[0].3;
        let moof_end = moof_offset + moof_size;
        let mut moof_off = moof_data_offset;
        let mut moof_boxes = Vec::new();
        while moof_off < moof_end {
            let (box_type, size, data_offset, data_len) = parse_box(&frag, moof_off).unwrap();
            println!("  Moof child {} at moof+{} size {}", std::str::from_utf8(box_type).unwrap(), moof_off - moof_offset, size);
            moof_boxes.push((box_type.to_vec(), moof_off, size, data_offset, data_len));
            moof_off += size;
        }

        // Should be mfhd + 2 trafs
        assert_eq!(moof_boxes.len(), 3);
        assert_eq!(&moof_boxes[0].0, b"mfhd");
        assert_eq!(&moof_boxes[1].0, b"traf");
        assert_eq!(&moof_boxes[2].0, b"traf");

        // Parse first traf (video)
        for (i, traf_box) in moof_boxes[1..].iter().enumerate() {
            let traf_offset = traf_box.1;
            let traf_size = traf_box.2;
            let traf_data_offset = traf_box.3;
            let traf_end = traf_offset + traf_size;
            let mut traf_off = traf_data_offset;
            let mut traf_children = Vec::new();
            while traf_off < traf_end {
                let (box_type, size, data_offset, data_len) = parse_box(&frag, traf_off).unwrap();
                println!("    Traf[{}] child {} at moof+{} size {}", i, std::str::from_utf8(box_type).unwrap(), traf_off - moof_offset, size);
                traf_children.push((box_type.to_vec(), traf_off, size, data_offset, data_len));
                traf_off += size;
            }

            // Should be tfhd + tfdt + trun
            assert_eq!(traf_children.len(), 3, "Traf[{}] should have tfhd+tfdt+trun", i);
            assert_eq!(&traf_children[0].0, b"tfhd");
            assert_eq!(&traf_children[1].0, b"tfdt");
            assert_eq!(&traf_children[2].0, b"trun");

            // Check tfhd
            let tfhd_offset = traf_children[0].3;
            let tfhd_version = frag[tfhd_offset];
            let tfhd_flags = u32::from_be_bytes([0, frag[tfhd_offset+1], frag[tfhd_offset+2], frag[tfhd_offset+3]]);
            let tfhd_track_id = u32::from_be_bytes([frag[tfhd_offset+4], frag[tfhd_offset+5], frag[tfhd_offset+6], frag[tfhd_offset+7]]);
            println!("      tfhd version={} flags=0x{:06X} track_id={}", tfhd_version, tfhd_flags, tfhd_track_id);
            assert_eq!(tfhd_version, 0);
            assert!(tfhd_flags & 0x020000 != 0, "default-base-is-moof should be set");
            assert!(tfhd_flags & 0x000008 != 0, "default-sample-duration-present");
            assert!(tfhd_flags & 0x000010 != 0, "default-sample-size-present");
            assert!(tfhd_flags & 0x000020 != 0, "default-sample-flags-present");
            assert_eq!(tfhd_track_id, if i == 0 { 1 } else { 2 });

            // Check tfdt
            let tfdt_offset = traf_children[1].3;
            let tfdt_version = frag[tfdt_offset];
            let tfdt_base = if tfdt_version == 1 {
                u64::from_be_bytes([
                    frag[tfdt_offset+4], frag[tfdt_offset+5], frag[tfdt_offset+6], frag[tfdt_offset+7],
                    frag[tfdt_offset+8], frag[tfdt_offset+9], frag[tfdt_offset+10], frag[tfdt_offset+11],
                ])
            } else {
                u32::from_be_bytes([frag[tfdt_offset+4], frag[tfdt_offset+5], frag[tfdt_offset+6], frag[tfdt_offset+7]]) as u64
            };
            println!("      tfdt version={} baseMediaDecodeTime={}", tfdt_version, tfdt_base);
            assert_eq!(tfdt_version, 1);
            assert_eq!(tfdt_base, 0);

            // Check trun
            let trun_offset = traf_children[2].3;
            let trun_version = frag[trun_offset];
            let trun_flags = u32::from_be_bytes([0, frag[trun_offset+1], frag[trun_offset+2], frag[trun_offset+3]]);
            let sample_count = u32::from_be_bytes([frag[trun_offset+4], frag[trun_offset+5], frag[trun_offset+6], frag[trun_offset+7]]);
            let data_offset_val = i32::from_be_bytes([frag[trun_offset+8], frag[trun_offset+9], frag[trun_offset+10], frag[trun_offset+11]]);
            println!("      trun version={} flags=0x{:06X} sample_count={} data_offset={}", trun_version, trun_flags, sample_count, data_offset_val);
            assert_eq!(trun_version, 0);
            assert_eq!(sample_count, 1);
            assert!(trun_flags & 0x000001 != 0, "data-offset-present");
            assert!(trun_flags & 0x000200 != 0, "sample-size-present");

            if i == 0 {
                // Video trun: first-sample-flags-present, no per-sample duration/flags
                assert!(trun_flags & 0x000004 != 0, "first-sample-flags-present");
                assert!(trun_flags & 0x000100 == 0, "video should not have sample-duration-present");
                assert!(trun_flags & 0x000400 == 0, "video should not have sample-flags-present");
            } else {
                // Audio trun: no per-sample duration (uses tfhd default), no per-sample flags
                assert!(trun_flags & 0x000100 == 0, "audio should not have sample-duration-present");
                assert!(trun_flags & 0x000004 == 0, "audio should not have first-sample-flags-present");
                assert!(trun_flags & 0x000400 == 0, "audio should not have sample-flags-present");
            }

            // Verify data_offset points into mdat payload.
            // default-base-is-moof means base_data_offset = moof start, so
            // data_offset is relative to moof start. For multi-track, each
            // track's data_offset skips previous tracks' sample data.
            let mdat_offset = boxes[1].1;
            let mdat_header = 8;
            let base_data_offset = (mdat_offset + mdat_header) as i32 - moof_offset as i32;
            let expected_data_offset = if i == 0 {
                base_data_offset
            } else {
                // skip previous track's samples (video = 8 bytes)
                base_data_offset + video_data.len() as i32
            };
            assert_eq!(data_offset_val, expected_data_offset, "data_offset should be relative to moof start");

            // Verify sample entry
            if i == 0 {
                // Video: first_sample_flags (4) + size (4) = 8 bytes before entries
                let first_flags = u32::from_be_bytes([frag[trun_offset+12], frag[trun_offset+13], frag[trun_offset+14], frag[trun_offset+15]]);
                let entry_off = trun_offset + 16;
                let size = u32::from_be_bytes([frag[entry_off], frag[entry_off+1], frag[entry_off+2], frag[entry_off+3]]);
                println!("      first_sample_flags=0x{:08X} size={}", first_flags, size);
                assert_eq!(first_flags, 0x02000000, "video keyframe first-sample-flags");
                assert_eq!(size, video_data.len() as u32);
            } else {
                // Audio: size (4) only; duration from tfhd default (no per-sample duration)
                let entry_off = trun_offset + 12;
                let size = u32::from_be_bytes([frag[entry_off], frag[entry_off+1], frag[entry_off+2], frag[entry_off+3]]);
                println!("      sample size={}", size);
                assert_eq!(size, audio_data.len() as u32);
            }
        }
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
