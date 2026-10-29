pub fn decode_cp936(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_passthrough() {
        assert_eq!(decode_cp936(b"hello world"), "hello world");
    }

    #[test]
    fn gbk_fallback() {
        let gbk = [0xC4, 0xE3, 0xBA, 0xC3];
        assert_eq!(decode_cp936(&gbk), "你好");
    }
}
