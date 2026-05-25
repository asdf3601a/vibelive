// Playlist generation helpers
// Core playlist logic is integrated in HlsStreamState.update_playlist()

pub struct Playlist {
    pub segments: Vec<SegmentInfo>,
    pub target_duration: u32,
    pub media_sequence: u32,
    pub is_live: bool,
}

pub struct SegmentInfo {
    pub duration: f64,
    pub filename: String,
    pub discontinuity: bool,
}

impl Playlist {
    pub fn new(target_duration: u32) -> Self {
        Self {
            segments: Vec::new(),
            target_duration,
            media_sequence: 0,
            is_live: true,
        }
    }

    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str("#EXTM3U\n");
        output.push_str("#EXT-X-VERSION:3\n");
        output.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", self.target_duration));
        output.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{}\n", self.media_sequence));

        if self.is_live {
            output.push_str("#EXT-X-DISCONTINUITY\n");
        }

        for segment in &self.segments {
            if segment.discontinuity {
                output.push_str("#EXT-X-DISCONTINUITY\n");
            }
            output.push_str(&format!("#EXTINF:{:.3},\n", segment.duration));
            output.push_str(&format!("{}\n", segment.filename));
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playlist_render() {
        let mut pl = Playlist::new(4);
        pl.segments.push(SegmentInfo {
            duration: 4.000,
            filename: "segment00000.ts".into(),
            discontinuity: true,
        });
        pl.segments.push(SegmentInfo {
            duration: 3.840,
            filename: "segment00001.ts".into(),
            discontinuity: false,
        });
        let output = pl.render();
        assert!(output.contains("#EXTM3U"));
        assert!(output.contains("segment00000.ts"));
        assert!(output.contains("segment00001.ts"));
        assert!(output.contains("#EXT-X-TARGETDURATION:4"));
    }
}