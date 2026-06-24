pub mod codec;

pub use codec::ensure_av1_obu_size_fields;

use codec::{av1c_box_from_config, build_dfla, build_dops, build_esds, parse_flac_streaminfo};
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

#[derive(Clone, Debug)]
pub struct ColorConfig {
    pub color_primaries: u16,
    pub transfer_characteristics: u16,
    pub matrix_coefficients: u16,
    pub full_range: bool,
}

#[derive(Clone, Debug)]
pub struct HdrMetadata {
    pub max_content_light_level: u16,
    pub max_frame_average_light_level: u16,
    pub display_primaries_x: [u16; 3],
    pub display_primaries_y: [u16; 3],
    pub white_point_x: u16,
    pub white_point_y: u16,
    pub max_luminance: u32,
    pub min_luminance: u32,
}

pub struct Fmp4Muxer {
    video_codec: Option<VideoCodec>,
    audio_codec: Option<AudioCodec>,
    video_width: u16,
    video_height: u16,
    video_config: Option<Vec<u8>>,
    audio_config: Option<Vec<u8>>,
    video_color_config: Option<ColorConfig>,
    hdr_metadata: Option<HdrMetadata>,
    video_samples: Vec<Sample>,
    audio_samples: Vec<Sample>,
    video_sequence_number: u32,
    video_base_dts: u64,
    // For audio codecs with fixed frame durations, keep decode time in the
    // audio media timescale (samples), not in RTMP milliseconds. AAC tfdt
    // values must land on the 1024-sample grid; converting rounded RTMP ms
    // back to samples produces offsets such as 29520432 % 1024 != 0.
    audio_base_dts: u64,
    audio_next_dts: u64,
    audio_sample_rate: u32,
    video_fps_num: u64,
    video_fps_den: u64,
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
            video_color_config: None,
            hdr_metadata: None,
            video_samples: Vec::new(),
            audio_samples: Vec::new(),
            video_sequence_number: 0,
            video_base_dts: 0,
            audio_base_dts: 0,
            audio_next_dts: 0,
            audio_sample_rate: 44100,
            video_fps_num: 30,
            video_fps_den: 1,
        }
    }

    pub fn set_video_framerate(&mut self, num: u64, den: u64) {
        self.video_fps_num = num;
        self.video_fps_den = den;
    }

    pub fn set_video_color_config(&mut self, cfg: ColorConfig) {
        self.video_color_config = Some(cfg);
    }

    pub fn set_hdr_metadata(&mut self, hdr: HdrMetadata) {
        self.hdr_metadata = Some(hdr);
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
        if let Some(codec) = self.audio_codec {
            match codec {
                AudioCodec::Aac => {
                    if config.len() >= 2 {
                        const AAC_SAMPLE_RATES: [u32; 16] = [
                            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000,
                            11025, 8000, 7350, 0, 0, 0,
                        ];
                        let rate_index =
                            ((config[0] as u32 & 0x07) << 1) | ((config[1] as u32 >> 7) & 0x01);
                        if (rate_index as usize) < AAC_SAMPLE_RATES.len() {
                            self.audio_sample_rate = AAC_SAMPLE_RATES[rate_index as usize];
                        }
                    }
                }
                AudioCodec::Opus => {
                    self.audio_sample_rate = 48000;
                }
                AudioCodec::Flac => {
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
        let dts_ts = dts;

        if self.video_samples.is_empty() {
            self.video_base_dts = dts_ts;
        }

        let size = data.len() as u32;
        let flags = if is_keyframe { 0x02000000 } else { 0x01010000 };
        let diff = pts as i64 - dts as i64;
        let cto_ts = if diff >= 0 {
            (diff * self.video_fps_num as i64 + 500) / 1000
        } else {
            (diff * self.video_fps_num as i64 - 500) / 1000
        } as i32;
        self.video_samples.push(Sample {
            data: data.into_owned(),
            dts: dts_ts,
            size,
            duration: 0,
            flags,
            composition_time_offset: cto_ts,
        });
    }

    pub fn add_audio_sample(&mut self, data: Cow<'_, [u8]>, pts: u64) {
        if data.is_empty() {
            return;
        }
        let dts_ts = if self.audio_codec.is_some() {
            self.audio_next_dts
        } else {
            // Unknown/legacy fallback keeps the old RTMP-ms based timeline so
            // duration inference can still use timestamp deltas.
            pts
        };
        if self.audio_samples.is_empty() {
            self.audio_base_dts = dts_ts;
        }
        let size = data.len() as u32;
        self.audio_samples.push(Sample {
            data: data.into_owned(),
            dts: dts_ts,
            size,
            duration: 0,
            flags: 0,
            composition_time_offset: 0,
        });
        if self.audio_codec.is_some() {
            self.audio_next_dts = self
                .audio_next_dts
                .saturating_add(self.audio_frame_duration_ticks() as u64);
        }
    }

    pub fn last_video_sample_duration(&self) -> u64 {
        if self.video_fps_num > 0 && self.video_fps_den > 0 {
            (self.video_fps_den * 1000 + self.video_fps_num / 2) / self.video_fps_num
        } else {
            33
        }
    }

    pub fn last_audio_sample_duration(&self) -> u64 {
        let ticks = self.audio_frame_duration_ticks() as u64;
        let rate = self.audio_sample_rate as u64;
        (ticks * 1000 + rate / 2).checked_div(rate).unwrap_or(21)
    }

    fn audio_frame_duration_ticks(&self) -> u32 {
        match self.audio_codec {
            Some(AudioCodec::Aac) => 1024,
            Some(AudioCodec::Opus) => {
                // FFmpeg's libopus produces non-standard TOC bytes (0xf8 for
                // standard 20ms frames) that cannot be reliably decoded with
                // any single bit layout. The OpusHead/dOps box is the
                // authoritative source for the stream's frame duration, but it
                // requires parsing the config packet which is not always
                // available at flush time.
                //
                // Since virtually all RTMP Opus streams use 20ms frames
                // (960 samples at 48kHz), hardcoding 960 is both simpler and
                // more reliable than per-packet TOC parsing. The 2.5% error
                // margin for non-20ms modes (e.g. 40ms = 1920 samples) is
                // negligible compared to the 5x error (4800 vs 960) from
                // misreading FFmpeg's TOC bytes.
                960
            }
            Some(AudioCodec::Flac) => 4096,
            _ => {
                if self.audio_samples.len() > 1 {
                    let total = self.audio_samples.last().unwrap().dts
                        - self.audio_samples.first().unwrap().dts;
                    let cnt = self.audio_samples.len() as u64 - 1;
                    ((total * self.audio_sample_rate as u64 + (cnt * 1000) / 2) / (cnt * 1000))
                        as u32
                } else {
                    1024
                }
            }
        }
    }

    fn compute_and_set_durations(&mut self) {
        // Video: compute per-sample duration from DTS deltas (ms → timescale ticks).
        // The last sample's duration uses the declared framerate as fallback.
        let default_dur = self.video_fps_den as u32;
        if self.video_samples.len() > 1 {
            for i in 0..self.video_samples.len() - 1 {
                let delta_ms = self.video_samples[i + 1].dts - self.video_samples[i].dts;
                self.video_samples[i].duration =
                    (((delta_ms * self.video_fps_num + 500) / 1000) as u32).max(1);
            }
            if let Some(last) = self.video_samples.last_mut() {
                last.duration = default_dur;
            }
        } else if let Some(s) = self.video_samples.first_mut() {
            s.duration = default_dur;
        }

        if !self.audio_samples.is_empty() {
            let dur = self.audio_frame_duration_ticks();
            for s in &mut self.audio_samples {
                s.duration = dur;
            }
        }
    }

    pub fn flush_combined_fragment(&mut self) -> Option<Vec<u8>> {
        if self.video_samples.is_empty() && self.audio_samples.is_empty() {
            return None;
        }
        self.compute_and_set_durations();
        self.video_sequence_number += 1;

        let mut tracks: Vec<(u32, u64, &[Sample])> = Vec::new();
        if !self.video_samples.is_empty() {
            tracks.push((1, self.video_base_dts, &self.video_samples));
        }
        if !self.audio_samples.is_empty() {
            tracks.push((2, self.audio_base_dts, &self.audio_samples));
        }

        let mut moof_buf = Vec::new();
        self.write_moof_multi(&mut moof_buf, self.video_sequence_number, &tracks);

        let mdat_payload_size: usize = tracks
            .iter()
            .map(|(_, _, samples)| samples.iter().map(|s| s.data.len()).sum::<usize>())
            .sum();
        let mdat_size = 8 + mdat_payload_size;

        let mut buf = Vec::new();
        buf.extend_from_slice(&moof_buf);
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

    // ── Init segment box writers ────────────────────────────────

    fn write_ftyp(&self, w: &mut Vec<u8>) {
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

        let mut ilst_data = Vec::new();
        let mut too_data = Vec::new();
        let tool_name = b"LivestreamServer";
        let mut data_data = Vec::new();
        data_data.extend_from_slice(&0u32.to_be_bytes());
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
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&1000u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0x00010000u32.to_be_bytes());
        data.extend_from_slice(&0x0100u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 10]);
        data.extend_from_slice(&[
            0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
        ]);
        data.extend_from_slice(&[0u8; 24]);
        let max_track_id = if self.audio_codec.is_some() { 2 } else { 1 };
        data.extend_from_slice(&(max_track_id + 1u32).to_be_bytes());
        write_fullbox(w, b"mvhd", 0, 0, &data);
    }

    fn write_video_trak(&self, w: &mut Vec<u8>) {
        let mut trak_data = Vec::new();
        self.write_tkhd(
            &mut trak_data,
            1,
            0,
            (self.video_width as u32) << 16,
            (self.video_height as u32) << 16,
        );
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
            elst_data.extend_from_slice(&2u32.to_be_bytes());
            elst_data.extend_from_slice(&0u32.to_be_bytes());
            elst_data.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());
            elst_data.extend_from_slice(&1u16.to_be_bytes());
            elst_data.extend_from_slice(&0u16.to_be_bytes());
            elst_data.extend_from_slice(&0u32.to_be_bytes());
            elst_data.extend_from_slice(&0u32.to_be_bytes());
            elst_data.extend_from_slice(&1u16.to_be_bytes());
            elst_data.extend_from_slice(&0u16.to_be_bytes());
        } else {
            elst_data.extend_from_slice(&1u32.to_be_bytes());
            elst_data.extend_from_slice(&0u32.to_be_bytes());
            elst_data.extend_from_slice(&0u32.to_be_bytes());
            elst_data.extend_from_slice(&1u16.to_be_bytes());
            elst_data.extend_from_slice(&0u16.to_be_bytes());
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
        data.extend_from_slice(&[
            0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
        ]);
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());
        write_fullbox(w, b"tkhd", 0, 0x000003, &data);
    }

    fn write_mdia_video(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_mdhd(&mut data, self.video_fps_num as u32);
        self.write_hdlr(&mut data, b"vide", b"VideoHandler\0");
        self.write_minf_video(&mut data);
        write_box(w, b"mdia", &data);
    }

    fn write_mdia_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        self.write_mdhd(&mut data, self.audio_sample_rate);
        self.write_hdlr(&mut data, b"soun", b"SoundHandler\0");
        self.write_minf_audio(&mut data);
        write_box(w, b"mdia", &data);
    }

    fn write_mdhd(&self, w: &mut Vec<u8>, timescale: u32) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&timescale.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0x55C4u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        write_fullbox(w, b"mdhd", 0, 0, &data);
    }

    fn write_hdlr(&self, w: &mut Vec<u8>, handler_type: &[u8; 4], name: &[u8]) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(handler_type);
        data.extend_from_slice(&[0u8; 12]);
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
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 6]);
        write_fullbox(w, b"vmhd", 0, 0x000001, &data);
    }

    fn write_smhd(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        write_fullbox(w, b"smhd", 0, 0, &data);
    }

    fn write_dinf(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        let mut dref_data = Vec::new();
        dref_data.extend_from_slice(&1u32.to_be_bytes());
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
        if self.audio_codec == Some(AudioCodec::Opus) {
            self.write_sgpd_opus(&mut data);
        }
        self.write_empty_stbl_box(&mut data, b"stts");
        self.write_empty_stbl_box(&mut data, b"stsc");
        self.write_stsz(&mut data);
        self.write_empty_stbl_box(&mut data, b"stco");
        write_box(w, b"stbl", &data);
    }

    fn write_sgpd_opus(&self, w: &mut Vec<u8>) {
        let roll_dist = self.opus_roll_distance();
        let mut data = Vec::new();
        data.extend_from_slice(b"roll");
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&(roll_dist as u16).to_be_bytes());
        write_fullbox(w, b"sgpd", 0, 0, &data);
    }

    fn opus_roll_distance(&self) -> i16 {
        let Some(ref config) = self.audio_config else {
            return -1;
        };
        let head = if config.len() > 8 && &config[..8] == b"OpusHead" {
            &config[8..]
        } else {
            config.as_slice()
        };
        if head.len() < 12 {
            return -1;
        }
        let pre_skip = u16::from_le_bytes([head[2], head[3]]);
        -(pre_skip as i16)
    }

    fn write_empty_stbl_box(&self, w: &mut Vec<u8>, box_type: &[u8; 4]) {
        let data = 0u32.to_be_bytes();
        write_fullbox(w, box_type, 0, 0, &data);
    }

    fn write_stsz(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        write_fullbox(w, b"stsz", 0, 0, &data);
    }

    fn write_stsd_video(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_be_bytes());
        match self.video_codec {
            Some(VideoCodec::H264) => self.write_visual_sample_entry(&mut data, b"avc1", b"avcC"),
            Some(VideoCodec::H265) => self.write_visual_sample_entry(&mut data, b"hvc1", b"hvcC"),
            Some(VideoCodec::AV1) => self.write_visual_sample_entry(&mut data, b"av01", b"av1C"),
            None => {}
        }
        write_fullbox(w, b"stsd", 0, 0, &data);
    }

    fn write_stsd_audio(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_be_bytes());
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

    // ── HDR metadata box writers ───────────────────────────────────

    fn write_colr(&self, w: &mut Vec<u8>) {
        let Some(ref cfg) = self.video_color_config else {
            return;
        };
        let mut data = Vec::new();
        data.extend_from_slice(b"nclx");
        data.extend_from_slice(&cfg.color_primaries.to_be_bytes());
        data.extend_from_slice(&cfg.transfer_characteristics.to_be_bytes());
        data.extend_from_slice(&cfg.matrix_coefficients.to_be_bytes());
        data.push(if cfg.full_range { 0x80 } else { 0x00 });
        write_box(w, b"colr", &data);
    }

    fn write_clli(&self, w: &mut Vec<u8>) {
        let Some(ref hdr) = self.hdr_metadata else {
            return;
        };
        let mut data = Vec::new();
        data.extend_from_slice(&hdr.max_content_light_level.to_be_bytes());
        data.extend_from_slice(&hdr.max_frame_average_light_level.to_be_bytes());
        write_box(w, b"clli", &data);
    }

    fn write_mdcv(&self, w: &mut Vec<u8>) {
        let Some(ref hdr) = self.hdr_metadata else {
            return;
        };
        let mut data = Vec::new();
        for &i in &[1, 2, 0] {
            data.extend_from_slice(&hdr.display_primaries_x[i].to_be_bytes());
            data.extend_from_slice(&hdr.display_primaries_y[i].to_be_bytes());
        }
        data.extend_from_slice(&hdr.white_point_x.to_be_bytes());
        data.extend_from_slice(&hdr.white_point_y.to_be_bytes());
        data.extend_from_slice(&hdr.max_luminance.to_be_bytes());
        data.extend_from_slice(&hdr.min_luminance.to_be_bytes());
        write_box(w, b"mdcv", &data);
    }

    // ── Consolidated visual sample entry ─────────────────────────

    fn write_visual_sample_entry(
        &self,
        w: &mut Vec<u8>,
        codec_fourcc: &[u8; 4],
        config_box_name: &[u8; 4],
    ) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]);
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(&self.video_width.to_be_bytes());
        data.extend_from_slice(&self.video_height.to_be_bytes());
        data.extend_from_slice(&0x00480000u32.to_be_bytes());
        data.extend_from_slice(&0x00480000u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes());

        let compressorname: [u8; 32] = if codec_fourcc == b"av01" {
            let mut buf = [0u8; 32];
            buf[0] = 10;
            buf[1..11].copy_from_slice(b"AOM Coding");
            buf
        } else {
            let mut buf = [0u8; 32];
            let cn = b"LivestreamServer\0";
            buf[0] = cn.len() as u8;
            buf[1..=cn.len()].copy_from_slice(cn);
            buf
        };
        data.extend_from_slice(&compressorname);
        data.extend_from_slice(&0x0018u16.to_be_bytes());
        data.extend_from_slice(&0xFFFFu16.to_be_bytes());

        // Codec-specific config box (av1C needs processing, avcC/hvcC pass through)
        let config_data = match (codec_fourcc, self.video_config.as_ref()) {
            (b"av01", Some(cfg)) => av1c_box_from_config(cfg),
            (_, Some(cfg)) => cfg.clone(),
            _ => Vec::new(),
        };
        write_box(&mut data, config_box_name, &config_data);

        let mut pasp_data = Vec::new();
        pasp_data.extend_from_slice(&1u32.to_be_bytes());
        pasp_data.extend_from_slice(&1u32.to_be_bytes());
        write_box(&mut data, b"pasp", &pasp_data);

        // HDR metadata boxes (colr, clli, mdcv) — required for correct PQ/HLG playback
        self.write_colr(&mut data);
        self.write_clli(&mut data);
        self.write_mdcv(&mut data);

        write_box(w, codec_fourcc, &data);
    }

    // ── Audio sample entries ─────────────────────────────────────

    fn write_mp4a_sample_entry(&self, w: &mut Vec<u8>) {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]);
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 8]);
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&16u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&(self.audio_sample_rate << 16).to_be_bytes());

        if let Some(ref config) = self.audio_config {
            let esds_data = build_esds(config);
            write_fullbox(&mut data, b"esds", 0, 0, &esds_data);
        } else {
            write_fullbox(&mut data, b"esds", 0, 0, &[]);
        }

        let mut btrt_data = Vec::new();
        btrt_data.extend_from_slice(&0u32.to_be_bytes());
        btrt_data.extend_from_slice(&128000u32.to_be_bytes());
        btrt_data.extend_from_slice(&128000u32.to_be_bytes());
        write_box(&mut data, b"btrt", &btrt_data);

        write_box(w, b"mp4a", &data);
    }

    fn write_opus_sample_entry(&self, w: &mut Vec<u8>) {
        let dops = self.audio_config.as_ref().map(|c| build_dops(c));
        let channel_count = dops
            .as_ref()
            .and_then(|d| {
                if d.len() > 1 && d[1] >= 1 {
                    Some(d[1])
                } else {
                    None
                }
            })
            .unwrap_or(2);

        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]);
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 8]);
        data.extend_from_slice(&(channel_count as u16).to_be_bytes());
        data.extend_from_slice(&16u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&(48000u32 << 16).to_be_bytes());

        match dops {
            Some(ref d) => write_box(&mut data, b"dOps", d),
            None => write_box(
                &mut data,
                b"dOps",
                &[
                    0x00,
                    channel_count,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                    0xBB,
                    0x80,
                    0x00,
                    0x00,
                    0x00,
                ],
            ),
        }

        write_box(w, b"Opus", &data);
    }

    fn write_flac_sample_entry(&self, w: &mut Vec<u8>) {
        let (sample_rate, channel_count) = self
            .audio_config
            .as_ref()
            .and_then(|c| parse_flac_streaminfo(c))
            .unwrap_or((44100u32, 1u16));

        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 6]);
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 8]);
        data.extend_from_slice(&channel_count.to_be_bytes());
        data.extend_from_slice(&16u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&(sample_rate << 16).to_be_bytes());

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
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        write_fullbox(w, b"trex", 0, 0, &data);
    }

    // ── Fragment writers ─────────────────────────────────────────

    fn write_moof_multi(
        &self,
        w: &mut Vec<u8>,
        sequence_number: u32,
        tracks: &[(u32, u64, &[Sample])],
    ) {
        let mut moof_data = Vec::new();
        self.write_mfhd(&mut moof_data, sequence_number);

        let mut moof_size = 8 + 16;
        for (track_id, _base_dts, samples) in tracks {
            moof_size += self.compute_traf_size(*track_id, samples);
        }

        let mdat_header_size = 8;
        let mut current_data_offset = moof_size + mdat_header_size;

        for (track_id, base_dts, samples) in tracks {
            let track_data_offset = current_data_offset;
            self.write_traf(
                &mut moof_data,
                *track_id,
                *base_dts,
                samples,
                track_data_offset as u32,
            );
            current_data_offset += samples.iter().map(|s| s.data.len()).sum::<usize>();
        }

        write_box(w, b"moof", &moof_data);
    }

    fn compute_traf_size(&self, track_id: u32, samples: &[Sample]) -> usize {
        let is_video = track_id == 1;
        let has_cto = samples.iter().any(|s| s.composition_time_offset != 0);
        let is_opus_audio = !is_video && self.audio_codec == Some(AudioCodec::Opus);

        let tfhd_size = 28;
        let mut trun_header = 12 + 4 + 4;
        if is_video {
            trun_header += 4;
        }

        let mut entry_size = 4;
        if has_cto {
            entry_size += 4;
        }

        let trun_size = trun_header + samples.len() * entry_size;
        let sbgp_size = if is_opus_audio { 28 } else { 0 };
        8 + tfhd_size + 20 + sbgp_size + trun_size
    }

    fn write_mfhd(&self, w: &mut Vec<u8>, sequence_number: u32) {
        let data = sequence_number.to_be_bytes();
        write_fullbox(w, b"mfhd", 0, 0, &data);
    }

    fn write_traf(
        &self,
        w: &mut Vec<u8>,
        track_id: u32,
        base_dts: u64,
        samples: &[Sample],
        data_offset: u32,
    ) {
        let is_video = track_id == 1;
        let is_opus_audio = !is_video && self.audio_codec == Some(AudioCodec::Opus);
        // Video DTS is stored in ms. Known audio codecs store DTS directly in
        // the audio media timescale so tfdt stays on the codec frame grid.
        let scaled_base_dts = if is_video {
            (base_dts * self.video_fps_num + 500) / 1000
        } else if self.audio_codec.is_some() {
            base_dts
        } else {
            (base_dts * self.audio_sample_rate as u64 + 500) / 1000
        };
        let scaled_duration = if is_video {
            self.video_fps_den as u32
        } else {
            self.audio_frame_duration_ticks()
        };
        let default_size = samples.first().map(|s| s.size).unwrap_or(0);
        let default_flags = if is_video { 0x01010000 } else { 0x02000000 };

        let mut traf_data = Vec::new();
        self.write_tfhd(
            &mut traf_data,
            track_id,
            scaled_duration,
            default_size,
            default_flags,
        );
        self.write_tfdt(&mut traf_data, scaled_base_dts);
        if is_opus_audio {
            self.write_sbgp_opus(&mut traf_data, samples.len() as u32);
        }
        self.write_trun(&mut traf_data, track_id, samples, data_offset);
        write_box(w, b"traf", &traf_data);
    }

    fn write_sbgp_opus(&self, w: &mut Vec<u8>, sample_count: u32) {
        let mut data = Vec::new();
        data.extend_from_slice(b"roll");
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&sample_count.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        write_fullbox(w, b"sbgp", 0, 0, &data);
    }

    fn write_tfhd(
        &self,
        w: &mut Vec<u8>,
        track_id: u32,
        default_duration: u32,
        default_size: u32,
        default_flags: u32,
    ) {
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
        let version: u8 = if has_cto { 1 } else { 0 };

        let mut flags: u32 = 0x000001;
        if is_video {
            flags |= 0x000004;
        }
        flags |= 0x000200;
        if has_cto {
            flags |= 0x000800;
        }

        let mut data = Vec::new();
        data.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        data.extend_from_slice(&data_offset.to_be_bytes());

        if is_video {
            let first_flags = samples.first().map(|s| s.flags).unwrap_or(0x02000000);
            data.extend_from_slice(&first_flags.to_be_bytes());
        }

        for s in samples {
            data.extend_from_slice(&s.size.to_be_bytes());
            if has_cto {
                data.extend_from_slice(&s.composition_time_offset.to_be_bytes());
            }
        }

        write_fullbox(w, b"trun", version, flags, &data);
    }
}

/// Parse the TOC byte (first byte) of an Opus packet to determine
/// the total number of PCM samples per frame at 48kHz.
///
/// Opus-in-ISOBMFF §4.3.4: Opus sample duration is the total of
/// frame sizes expressed in seconds × media timescale.
/// At 48000Hz, each 20ms frame is 960 samples.
///
/// TOC byte layout per RFC 6716 §3.1 figure (errata-corrected):
///   bits 0-4: config (5 bits) — operating mode / frame size
///   bit  5:   stereo flag
///   bits 6-7: frame count code (2 bits)
///
/// In byte form (MSB = bit 7): config = (toc >> 3) & 0x1F, code = toc & 0x03.
/// For CELT-only modes (config 16-31), the base frame size matches config & 7.
///
/// NOTE: FFmpeg's libopus produces non-standard TOC bytes (e.g. 0xf8 for
/// standard 20ms frames). Because TOC parsing is unreliable for FFmpeg's
/// output, the caller (`audio_frame_duration_ticks`) hardcodes 960 for Opus.
/// This function remains for informational use only.
#[allow(dead_code)]
fn opus_frame_samples(packet: &[u8]) -> Option<u32> {
    let toc = *packet.first()?;
    let config_raw = (toc >> 3) & 0x1F;
    let config_base = config_raw & 0x07;
    let code = toc & 0x03;

    // Samples per frame at 48kHz for each base config value (RFC 6716 §3.1)
    let samples_per_frame: u32 = match config_base {
        0 => 120,
        1 => 240,
        2 => 480,
        3 => 960,
        4 => 1920,
        5 => 2880,
        6 => 3840,
        7 => 4800,
        _ => return None,
    };

    // Frame count from 2-bit code (RFC 6716 §3.2.3)
    let frame_count: u32 = match code {
        0 => 1,
        1 | 2 => 2,
        3 => 3,
        _ => 1,
    };

    Some(samples_per_frame * frame_count)
}

// ── Box helpers ──────────────────────────────────────────────────

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

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ftyp_box() {
        let muxer = Fmp4Muxer::new();
        let init = muxer.init_segment();
        assert!(!init.is_empty());
        assert_eq!(&init[4..8], b"ftyp");
        assert_eq!(&init[8..12], b"cmfc");
        assert!(init.windows(4).any(|w| w == b"iso6"));
    }

    #[test]
    fn test_moov_structure() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00]);
        let init = muxer.init_segment();
        assert!(init.windows(4).any(|w| w == b"moov"));
        assert!(init.windows(4).any(|w| w == b"mvhd"));
        assert!(init.windows(4).any(|w| w == b"trak"));
        assert!(init.windows(4).any(|w| w == b"mvex"));
    }

    fn read_mvhd_next_track_id(data: &[u8]) -> u32 {
        // Walk top-level boxes, find moov, then walk moov children to find mvhd
        let mut off = 0;
        while off + 8 <= data.len() {
            let box_size =
                u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    as usize;
            if off + box_size > data.len() {
                break;
            }
            if &data[off + 4..off + 8] == b"moov" {
                let moov_end = off + box_size;
                let mut inner = off + 8;
                while inner + 8 <= moov_end {
                    let child_size = u32::from_be_bytes([
                        data[inner],
                        data[inner + 1],
                        data[inner + 2],
                        data[inner + 3],
                    ]) as usize;
                    if inner + child_size > moov_end {
                        break;
                    }
                    if &data[inner + 4..inner + 8] == b"mvhd" {
                        // next_track_ID is at offset 92 from FullBox data start
                        let data_off = inner + 12; // skip size(4)+type(4)+version(1)+flags(3)
                        return u32::from_be_bytes([
                            data[data_off + 92],
                            data[data_off + 93],
                            data[data_off + 94],
                            data[data_off + 95],
                        ]);
                    }
                    inner += child_size;
                }
            }
            off += box_size;
        }
        panic!("mvhd not found");
    }

    #[test]
    fn test_mvhd_next_track_id_video_audio() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1280, 720);
        muxer.set_audio_codec(AudioCodec::Aac);
        let init = muxer.init_segment();
        assert_eq!(read_mvhd_next_track_id(&init), 3);
    }

    #[test]
    fn test_mvhd_next_track_id_video_only() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1280, 720);
        let init = muxer.init_segment();
        assert_eq!(read_mvhd_next_track_id(&init), 2);
    }

    #[test]
    fn test_mvhd_next_track_id_audio_only() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Aac);
        let init = muxer.init_segment();
        assert_eq!(read_mvhd_next_track_id(&init), 3);
    }

    #[test]
    fn test_video_fragment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);

        muxer.add_video_sample(vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88].into(), 0, 0, true);

        let frag = muxer.flush_combined_fragment();
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

        let frag = muxer.flush_combined_fragment().unwrap();
        assert!(frag.len() > 16);
        assert!(frag.windows(4).any(|w| w == b"moof"));
        assert!(frag.windows(4).any(|w| w == b"mdat"));
    }

    fn parse_box(data: &[u8], offset: usize) -> Option<(&[u8], usize, usize, usize)> {
        if offset + 8 > data.len() {
            return None;
        }
        let size = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        Some((
            &data[offset + 4..offset + 8],
            size,
            offset + 8,
            size.saturating_sub(8),
        ))
    }

    /// Walk ISOBMFF box hierarchy to find a child box within a traf within a moof.
    fn find_traf_child<'a>(
        frag: &'a [u8],
        traf_index: usize,
        child: &[u8; 4],
    ) -> (usize, usize, usize, &'a [u8]) {
        let (_type, moof_size, moof_data, _) = parse_box(frag, 0).unwrap();
        assert_eq!(_type, b"moof");
        let moof_end = moof_size;
        let mut off = moof_data;
        let mut traf_idx = 0;
        while off < moof_end {
            let (btype, bsize, bdata, _) = parse_box(frag, off).unwrap();
            if btype == b"traf" {
                if traf_idx == traf_index {
                    let traf_end = off + bsize;
                    let mut inner = bdata;
                    while inner < traf_end {
                        let (it, isize, idata, idata_len) = parse_box(frag, inner).unwrap();
                        if it == child {
                            return (inner, isize, idata, &frag[idata..idata + idata_len]);
                        }
                        inner += isize;
                    }
                }
                traf_idx += 1;
            }
            off += bsize;
        }
        panic!(
            "{} not found in traf[{}]",
            String::from_utf8_lossy(child),
            traf_index
        );
    }

    /// Walk ISOBMFF box hierarchy to find a trun within a traf within a moof,
    /// and return the `sample_count` field.
    fn get_trun_sample_count(frag: &[u8], traf_index: usize) -> u32 {
        let (_, _, _, trun_data) = find_traf_child(frag, traf_index, b"trun");
        u32::from_be_bytes([trun_data[4], trun_data[5], trun_data[6], trun_data[7]])
    }

    fn get_tfdt_base(frag: &[u8], traf_index: usize) -> u64 {
        let (_, _, _, tfdt_data) = find_traf_child(frag, traf_index, b"tfdt");
        assert_eq!(tfdt_data[0], 1, "tfdt should be version 1");
        u64::from_be_bytes([
            tfdt_data[4],
            tfdt_data[5],
            tfdt_data[6],
            tfdt_data[7],
            tfdt_data[8],
            tfdt_data[9],
            tfdt_data[10],
            tfdt_data[11],
        ])
    }

    #[test]
    fn test_large_multi_sample_fragment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);
        muxer.set_audio_config(vec![0x12, 0x10]);

        let video_data = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00];
        let audio_data = vec![0xAF, 0x01];

        for i in 0..60 {
            muxer.add_video_sample(
                video_data.clone().into(),
                i as u64 * 33,
                i as u64 * 33,
                i % 30 == 0,
            );
        }
        for i in 0..93 {
            muxer.add_audio_sample(audio_data.clone().into(), i as u64 * 21);
        }

        let frag = muxer.flush_combined_fragment().unwrap();

        let (_type, _size, _data, _) = parse_box(&frag, 0).unwrap();
        assert_eq!(_type, b"moof");
        let (next_type, _next_size, _, _) = parse_box(&frag, _size).unwrap();
        assert_eq!(next_type, b"mdat");

        assert_eq!(get_trun_sample_count(&frag, 0), 60);
        assert_eq!(get_trun_sample_count(&frag, 1), 93);
    }

    #[test]
    fn test_combined_fragment_box_structure() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);
        muxer.set_audio_config(vec![0x12, 0x10]);

        let video_data = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00];
        let audio_data = vec![0xAF, 0x01];
        muxer.add_video_sample(video_data.clone().into(), 0, 0, true);
        muxer.add_audio_sample(audio_data.clone().into(), 0);

        let frag = muxer.flush_combined_fragment().unwrap();

        // Parse top-level boxes
        let mut offset = 0;
        let mut boxes = Vec::new();
        while offset < frag.len() {
            let (box_type, size, data_offset, data_len) = parse_box(&frag, offset).unwrap();
            boxes.push((box_type.to_vec(), offset, size, data_offset, data_len));
            offset += size;
        }

        assert_eq!(boxes.len(), 2);
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
            moof_boxes.push((box_type.to_vec(), moof_off, size, data_offset, data_len));
            moof_off += size;
        }

        assert_eq!(moof_boxes.len(), 3);
        assert_eq!(&moof_boxes[0].0, b"mfhd");
        assert_eq!(&moof_boxes[1].0, b"traf");
        assert_eq!(&moof_boxes[2].0, b"traf");

        // Parse each traf
        for (i, traf_box) in moof_boxes[1..].iter().enumerate() {
            let traf_offset = traf_box.1;
            let traf_size = traf_box.2;
            let traf_data_offset = traf_box.3;
            let traf_end = traf_offset + traf_size;
            let mut traf_off = traf_data_offset;
            let mut traf_children = Vec::new();
            while traf_off < traf_end {
                let (box_type, size, data_offset, data_len) = parse_box(&frag, traf_off).unwrap();
                traf_children.push((box_type.to_vec(), traf_off, size, data_offset, data_len));
                traf_off += size;
            }

            assert_eq!(traf_children.len(), 3);
            assert_eq!(&traf_children[0].0, b"tfhd");
            assert_eq!(&traf_children[1].0, b"tfdt");
            assert_eq!(&traf_children[2].0, b"trun");

            // Check tfhd
            let tfhd_offset = traf_children[0].3;
            let tfhd_version = frag[tfhd_offset];
            let tfhd_flags = u32::from_be_bytes([
                0,
                frag[tfhd_offset + 1],
                frag[tfhd_offset + 2],
                frag[tfhd_offset + 3],
            ]);
            let tfhd_track_id = u32::from_be_bytes([
                frag[tfhd_offset + 4],
                frag[tfhd_offset + 5],
                frag[tfhd_offset + 6],
                frag[tfhd_offset + 7],
            ]);
            assert_eq!(tfhd_version, 0);
            assert!(tfhd_flags & 0x020000 != 0);
            assert!(tfhd_flags & 0x000008 != 0);
            assert!(tfhd_flags & 0x000010 != 0);
            assert!(tfhd_flags & 0x000020 != 0);
            assert_eq!(tfhd_track_id, if i == 0 { 1 } else { 2 });

            // Check tfdt
            let tfdt_offset = traf_children[1].3;
            let tfdt_version = frag[tfdt_offset];
            let tfdt_base = if tfdt_version == 1 {
                u64::from_be_bytes([
                    frag[tfdt_offset + 4],
                    frag[tfdt_offset + 5],
                    frag[tfdt_offset + 6],
                    frag[tfdt_offset + 7],
                    frag[tfdt_offset + 8],
                    frag[tfdt_offset + 9],
                    frag[tfdt_offset + 10],
                    frag[tfdt_offset + 11],
                ])
            } else {
                u32::from_be_bytes([
                    frag[tfdt_offset + 4],
                    frag[tfdt_offset + 5],
                    frag[tfdt_offset + 6],
                    frag[tfdt_offset + 7],
                ]) as u64
            };
            assert_eq!(tfdt_version, 1);
            assert_eq!(tfdt_base, 0);

            // Check trun
            let trun_offset = traf_children[2].3;
            let trun_version = frag[trun_offset];
            let trun_flags = u32::from_be_bytes([
                0,
                frag[trun_offset + 1],
                frag[trun_offset + 2],
                frag[trun_offset + 3],
            ]);
            let sample_count = u32::from_be_bytes([
                frag[trun_offset + 4],
                frag[trun_offset + 5],
                frag[trun_offset + 6],
                frag[trun_offset + 7],
            ]);
            let data_offset_val = i32::from_be_bytes([
                frag[trun_offset + 8],
                frag[trun_offset + 9],
                frag[trun_offset + 10],
                frag[trun_offset + 11],
            ]);
            assert_eq!(trun_version, 0);
            assert_eq!(sample_count, 1);
            assert!(trun_flags & 0x000001 != 0);
            assert!(trun_flags & 0x000200 != 0);

            if i == 0 {
                assert!(trun_flags & 0x000004 != 0);
                assert!(trun_flags & 0x000100 == 0);
                assert!(trun_flags & 0x000400 == 0);
            } else {
                assert!(trun_flags & 0x000100 == 0);
                assert!(trun_flags & 0x000004 == 0);
                assert!(trun_flags & 0x000400 == 0);
            }

            let mdat_offset = boxes[1].1;
            let mdat_header = 8;
            let base_data_offset = (mdat_offset + mdat_header) as i32 - moof_offset as i32;
            let expected_data_offset = if i == 0 {
                base_data_offset
            } else {
                base_data_offset + video_data.len() as i32
            };
            assert_eq!(data_offset_val, expected_data_offset);

            if i == 0 {
                let first_flags = u32::from_be_bytes([
                    frag[trun_offset + 12],
                    frag[trun_offset + 13],
                    frag[trun_offset + 14],
                    frag[trun_offset + 15],
                ]);
                let entry_off = trun_offset + 16;
                let size = u32::from_be_bytes([
                    frag[entry_off],
                    frag[entry_off + 1],
                    frag[entry_off + 2],
                    frag[entry_off + 3],
                ]);
                assert_eq!(first_flags, 0x02000000);
                assert_eq!(size, video_data.len() as u32);
            } else {
                let entry_off = trun_offset + 12;
                let size = u32::from_be_bytes([
                    frag[entry_off],
                    frag[entry_off + 1],
                    frag[entry_off + 2],
                    frag[entry_off + 3],
                ]);
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
        assert_eq!(init.windows(4).filter(|w| *w == b"trak").count(), 1);
        assert!(init.windows(4).any(|w| w == b"avc1"));
    }

    #[test]
    fn test_opus_sample_entry() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Opus);
        let config = vec![
            b'O', b'p', b'u', b's', b'H', b'e', b'a', b'd', 0x01, 0x02, 0x38, 0x01, 0x80, 0xBB,
            0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        muxer.set_audio_config(config);
        let init = muxer.init_segment();
        assert!(init.windows(4).any(|w| w == b"Opus"));
        assert!(init.windows(4).any(|w| w == b"dOps"));
    }

    #[test]
    fn test_audio_only_fragment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_audio_config(vec![0x12, 0x10]);

        muxer.add_audio_sample(vec![0xAF, 0x01].into(), 0);
        muxer.add_audio_sample(vec![0xBF, 0x02].into(), 21);

        let frag = muxer.flush_combined_fragment().unwrap();
        assert!(frag.windows(4).any(|w| w == b"moof"));
        assert!(frag.windows(4).any(|w| w == b"mdat"));
    }

    #[test]
    fn test_aac_tfdt_stays_on_1024_sample_grid_across_fragments() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_audio_config(vec![0x11, 0x90]); // AAC-LC, 48kHz, stereo

        // These are typical RTMP millisecond timestamps for exact 1024-sample
        // AAC frames at 48kHz.  Converting each ms timestamp back to samples
        // would produce 1008, 2064, ... and eventually non-1024-aligned tfdt.
        muxer.add_audio_sample(vec![0xAF, 0x01].into(), 0);
        muxer.add_audio_sample(vec![0xAF, 0x02].into(), 21);
        muxer.add_audio_sample(vec![0xAF, 0x03].into(), 43);
        let frag0 = muxer.flush_combined_fragment().unwrap();
        assert_eq!(get_tfdt_base(&frag0, 0), 0);

        muxer.add_audio_sample(vec![0xAF, 0x04].into(), 64);
        muxer.add_audio_sample(vec![0xAF, 0x05].into(), 85);
        let frag1 = muxer.flush_combined_fragment().unwrap();
        let base = get_tfdt_base(&frag1, 0);
        assert_eq!(base, 3 * 1024);
        assert_eq!(base % 1024, 0);
    }

    #[test]
    fn test_hevc_init_segment() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H265, 1920, 1080);
        muxer.set_video_config(vec![
            0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0xF0,
            0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 0x03, 0x20, 0x00, 0x00, 0x03, 0x00,
            0x80, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x78, 0xAC, 0x09,
        ]);
        let init = muxer.init_segment();
        assert!(init.windows(4).any(|w| w == b"hvc1"));
        assert!(init.windows(4).any(|w| w == b"hvcC"));
    }

    #[test]
    fn test_trun_version_1_with_composition_offset() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_video_codec(VideoCodec::H264, 1920, 1080);
        muxer.set_video_config(vec![0x01, 0x42, 0xC0, 0x1E]);

        // Sample with DTS=0, PTS=100  →  composition_time_offset = 100*30/1000 = 3
        muxer.add_video_sample(vec![0x00, 0x00, 0x00, 0x01, 0x65].into(), 0, 100, true);

        let frag = muxer.flush_combined_fragment().unwrap();

        // Find trun version inside moof/traf
        let (_type, moof_size, moof_data, _) = parse_box(&frag, 0).unwrap();
        assert_eq!(_type, b"moof");
        let mut off = moof_data;
        while off < moof_size {
            let (btype, bsize, bdata, _) = parse_box(&frag, off).unwrap();
            if btype == b"traf" {
                let traf_end = off + bsize;
                let mut inner = bdata;
                while inner < traf_end {
                    let (it, _isize, idata, _) = parse_box(&frag, inner).unwrap();
                    if it == b"trun" {
                        let version = frag[idata];
                        assert_eq!(
                            version, 1,
                            "trun version should be 1 when composition_time_offset is non-zero"
                        );
                        // Check CTO flag is set (0x000800)
                        let trun_flags = u32::from_be_bytes([
                            0,
                            frag[idata + 1],
                            frag[idata + 2],
                            frag[idata + 3],
                        ]);
                        assert!(trun_flags & 0x000800 != 0, "trun should have sample_composition_time_offset flag");
                        // trun data: version(1)+flags(3)+sample_count(4)+data_offset(4)+first_flags(4)+entries
                        // Each entry: sample_size(4)+composition_time_offset(4)
                        // CTO is at data_offset+20 for video
                        // CTO is in media timescale: 100ms X 30 / 1000 = 3
                        let cto = i32::from_be_bytes([
                            frag[idata + 20],
                            frag[idata + 21],
                            frag[idata + 22],
                            frag[idata + 23],
                        ]);
                        assert_eq!(cto, 3);
                        return;
                    }
                    inner += _isize;
                }
            }
            off += bsize;
        }
        panic!("trun not found in moof");
    }

    #[test]
    fn test_flac_sample_entry() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Flac);
        let config = vec![
            0x66, 0x4c, 0x61, 0x43, 0x12, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x15,
            0x0a, 0xc4, 0x40, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(config.len(), 38);
        muxer.set_audio_config(config);
        let init = muxer.init_segment();
        assert!(init.windows(4).any(|w| w == b"fLaC"));
        assert!(init.windows(4).any(|w| w == b"dfLa"));

        let dfla_pos = init.windows(4).position(|w| w == b"dfLa").unwrap();
        let dfla_size = u32::from_be_bytes([
            init[dfla_pos - 4],
            init[dfla_pos - 3],
            init[dfla_pos - 2],
            init[dfla_pos - 1],
        ]) as usize;
        let dfla_data = &init[dfla_pos + 4..dfla_pos - 4 + dfla_size];
        assert_eq!(dfla_data[0], 0x00);
        assert_eq!(&dfla_data[1..4], &[0x00, 0x00, 0x00]);
        assert_eq!(dfla_data[4], 0x80);
        assert_eq!(
            u32::from_be_bytes([0x00, dfla_data[5], dfla_data[6], dfla_data[7]]),
            34
        );
        assert_eq!(
            &dfla_data[8..42],
            &[
                0x12, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x15, 0x0a, 0xc4, 0x40, 0xf0,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn test_opus_frame_samples_all_configs() {
        // config=0 (2.5ms), code=0 (1 frame) → 120 samples
        // TOC: config=0<<3 | code=0 = 0x00
        assert_eq!(opus_frame_samples(&[0x00]), Some(120));

        // config=3 (20ms, most common), code=0 (1 frame) → 960 samples
        // TOC: config=3<<3 | code=0 = 0x18
        assert_eq!(opus_frame_samples(&[0x18]), Some(960));

        // config=3, stereo=1, code=1 (2 frames) → 2 × 960 = 1920 samples
        // TOC: config=3<<3 | stereo=1<<2 | code=1 = 0x18 | 0x04 | 0x01 = 0x1D
        assert_eq!(opus_frame_samples(&[0x1D]), Some(1920));

        // config=4 (40ms), code=0 (1 frame) → 1920 samples
        // TOC: config=4<<3 | code=0 = 0x20
        assert_eq!(opus_frame_samples(&[0x20]), Some(1920));

        // config=3, code=3 (3 frames) → 3 × 960 = 2880 samples
        // TOC: config=3<<3 | code=3 = 0x1B
        assert_eq!(opus_frame_samples(&[0x1B]), Some(2880));

        // Empty packet → None
        assert!(opus_frame_samples(&[]).is_none());
    }

    #[test]
    fn test_audio_duration_aac_fixed_1024() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_audio_config(vec![0x12, 0x10]); // 44100Hz

        // Feed samples at jittery RTMP timestamps that would produce
        // an incorrect average with the old RTMP-averaging approach.
        muxer.add_audio_sample(vec![0xAF, 0x01].into(), 0);
        muxer.add_audio_sample(vec![0xAF, 0x02].into(), 23);
        muxer.add_audio_sample(vec![0xAF, 0x03].into(), 46);
        muxer.add_audio_sample(vec![0xAF, 0x04].into(), 70); // 24ms gap instead of 23

        muxer.compute_and_set_durations();

        // Every audio sample must have exactly 1024 (AAC fixed frame size)
        for s in &muxer.audio_samples {
            assert_eq!(s.duration, 1024, "AAC sample duration must be 1024, got {}", s.duration);
        }
    }

    #[test]
    fn test_audio_duration_opus_from_toc() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Opus);
        muxer.set_audio_config(vec![0x01, 0x02, 0x38, 0x01, 0x80, 0xBB, 0x00, 0x00, 0x00, 0x00, 0x00]);

        // Opus packet with TOC: config=3 (20ms), c=0 (1 frame) → 960 samples
        // TOC = (3<<5) | 0 = 0x60
        let toc: u8 = 0x60;
        let packet = vec![toc, 0x00, 0x01, 0x02];
        muxer.add_audio_sample(packet.into(), 0);

        let packet2 = vec![toc, 0x03, 0x04, 0x05];
        muxer.add_audio_sample(packet2.into(), 20);

        muxer.compute_and_set_durations();

        for s in &muxer.audio_samples {
            assert_eq!(s.duration, 960, "Opus sample duration must be 960 (20ms @ 48kHz), got {}", s.duration);
        }
    }

    #[test]
    fn test_audio_duration_opus_fallback_on_empty_packet() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Opus);
        muxer.set_audio_config(vec![0x01, 0x02, 0x38, 0x01]);

        // Empty audio packet → opus_frame_samples returns None → fallback to 960
        muxer.add_audio_sample(vec![].into(), 0);
        muxer.add_audio_sample(vec![].into(), 20);

        muxer.compute_and_set_durations();

        for s in &muxer.audio_samples {
            assert_eq!(s.duration, 960, "Opus fallback duration must be 960, got {}", s.duration);
        }
    }

    #[test]
    fn test_audio_duration_flac_single_4096() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Flac);
        muxer.set_audio_config(vec![
            0x66, 0x4c, 0x61, 0x43, 0x12, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24, 0x15,
            0x0a, 0xc4, 0x40, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);

        // Single FLAC sample → fallback to 4096
        muxer.add_audio_sample(vec![0xFF, 0xF0, 0x00, 0x01].into(), 0);

        muxer.compute_and_set_durations();

        assert_eq!(muxer.audio_samples.len(), 1);
        assert_eq!(muxer.audio_samples[0].duration, 4096,
            "Single FLAC sample should default to 4096");
    }

    #[test]
    fn test_audio_duration_unknown_codec_no_audio_codec_set() {
        // When audio_codec is None (not set), the old RTMP averaging should be used.
        let mut muxer = Fmp4Muxer::new();
        // Deliberately NOT setting audio_codec
        muxer.set_audio_config(vec![0x12, 0x10]);

        muxer.add_audio_sample(vec![0xAF, 0x01].into(), 0);
        muxer.add_audio_sample(vec![0xAF, 0x02].into(), 23);

        muxer.compute_and_set_durations();

        // With audio_codec=None, the code uses the RTMP averaging branch
        // which set_audio_config set sample_rate to 44100.
        // total=23, cnt=1 → 23*44100/1000 = 1014 (rounded from 1014.3)
        assert_eq!(muxer.audio_samples[0].duration, 1014);
        assert_eq!(muxer.audio_samples[0].duration, muxer.audio_samples[1].duration);
    }

    #[test]
    fn test_last_audio_sample_duration_rounding() {
        let mut muxer = Fmp4Muxer::new();
        muxer.set_audio_codec(AudioCodec::Aac);
        muxer.set_audio_config(vec![0x12, 0x10]); // 44100Hz

        // AAC duration = 1024 samples @ 44100Hz
        // Expected ms = round(1024 * 1000 / 44100) = round(23.219) = 23
        muxer.add_audio_sample(vec![0xAF, 0x01].into(), 0);

        // Call compute_and_set_durations to set audio duration fields
        muxer.compute_and_set_durations();

        let ms = muxer.last_audio_sample_duration();
        // 1024 * 1000 / 44100 = 23 (truncated) but with rounding: (1024*1000 + 22050) / 44100 = 1024066.5 / 44100 = 23
        assert_eq!(ms, 23, "AAC duration at 44100Hz should round to 23ms");
    }

    // ── HDR box validation ──────────────────────────────────────────

    fn find_box_bytes<'a>(data: &'a [u8], target: &[u8; 4]) -> Option<(usize, &'a [u8])> {
        let mut off = 0;
        while off + 8 <= data.len() {
            let size =
                u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    as usize;
            if size == 0 || off + size > data.len() {
                break;
            }
            if &data[off + 4..off + 8] == target {
                return Some((size, &data[off + 8..off + size]));
            }
            let fourcc = match data.get(off + 4..off + 8) {
                Some(s) if s.len() == 4 => {
                    let mut a = [0u8; 4];
                    a.copy_from_slice(s);
                    a
                }
                _ => {
                    off += size;
                    continue;
                }
            };
            match &fourcc {
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" if size > 8 => {
                    if let Some(r) = find_box_bytes(&data[off + 8..off + size], target) {
                        return Some(r);
                    }
                }
                b"stsd" if size > 16 => {
                    if let Some(r) = find_box_bytes(&data[off + 16..off + size], target) {
                        return Some(r);
                    }
                }
                b"hvc1" | b"avc1" | b"av01" | b"hev1" if size > 8 => {
                    if let Some(r) = find_box_bytes(&data[off + 86..off + size], target) {
                        return Some(r);
                    }
                }
                _ => {}
            }
            off += size;
        }
        None
    }

    #[test]
    fn test_hdr_boxes_clli_mdcv_in_init_segment() {
        let mut mux = Fmp4Muxer::new();
        mux.set_video_codec(VideoCodec::H265, 1920, 1080);
        mux.set_audio_codec(AudioCodec::Aac);
        mux.set_video_config(vec![
            0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78,
            0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 0x03, 0x20, 0x00, 0x00,
            0x03, 0x00, 0x80, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x78, 0xAC, 0x09,
        ]);
        mux.set_audio_config(vec![0x12, 0x10]);

        mux.set_video_color_config(ColorConfig {
            color_primaries: 9,
            transfer_characteristics: 16,
            matrix_coefficients: 9,
            full_range: false,
        });

        // Mastering display: Display P3 primaries, 1000 nits peak, 0.005 nits black
        mux.set_hdr_metadata(HdrMetadata {
            max_content_light_level: 1000,
            max_frame_average_light_level: 400,
            display_primaries_x: [34000, 13250, 7500],
            display_primaries_y: [16000, 34500, 3000],
            white_point_x: 15635,
            white_point_y: 16450,
            max_luminance: 10_000_000,
            min_luminance: 50,
        });

        let init = mux.init_segment();

        let (clli_size, clli_data) =
            find_box_bytes(&init, b"clli").expect("clli box missing from init segment");
        assert_eq!(clli_size, 12, "clli box should be 12 bytes");
        assert_eq!(
            u16::from_be_bytes([clli_data[0], clli_data[1]]),
            1000,
            "MaxCLL"
        );
        assert_eq!(
            u16::from_be_bytes([clli_data[2], clli_data[3]]),
            400,
            "MaxFALL"
        );

        let (mdcv_size, mdcv_data) =
            find_box_bytes(&init, b"mdcv").expect("mdcv box missing from init segment");
        assert_eq!(mdcv_size, 32, "mdcv box should be 32 bytes");

        assert_eq!(u16::from_be_bytes([mdcv_data[0], mdcv_data[1]]), 13250, "GX");
        assert_eq!(u16::from_be_bytes([mdcv_data[2], mdcv_data[3]]), 34500, "GY");
        assert_eq!(u16::from_be_bytes([mdcv_data[4], mdcv_data[5]]), 7500, "BX");
        assert_eq!(u16::from_be_bytes([mdcv_data[6], mdcv_data[7]]), 3000, "BY");
        assert_eq!(u16::from_be_bytes([mdcv_data[8], mdcv_data[9]]), 34000, "RX");
        assert_eq!(u16::from_be_bytes([mdcv_data[10], mdcv_data[11]]), 16000, "RY");
        assert_eq!(u16::from_be_bytes([mdcv_data[12], mdcv_data[13]]), 15635, "WX");
        assert_eq!(u16::from_be_bytes([mdcv_data[14], mdcv_data[15]]), 16450, "WY");
        assert_eq!(
            u32::from_be_bytes([mdcv_data[16], mdcv_data[17], mdcv_data[18], mdcv_data[19]]),
            10_000_000,
            "maxLum"
        );
        assert_eq!(
            u32::from_be_bytes([mdcv_data[20], mdcv_data[21], mdcv_data[22], mdcv_data[23]]),
            50,
            "minLum"
        );
    }
}
