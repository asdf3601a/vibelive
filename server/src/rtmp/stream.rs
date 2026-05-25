use uuid::Uuid;

pub struct StreamKeyInfo {
    pub stream_id: Uuid,
    pub stream_key: String,
}

pub fn validate_stream_key(_stream_key: &str) -> Option<StreamKeyInfo> {
    // Any stream key is accepted; generate a synthetic stream_id for internal use
    Some(StreamKeyInfo {
        stream_id: Uuid::new_v4(),
        stream_key: _stream_key.to_string(),
    })
}
