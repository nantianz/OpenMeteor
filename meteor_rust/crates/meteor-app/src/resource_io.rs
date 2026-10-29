pub(crate) fn read(path: impl AsRef<std::path::Path>) -> std::io::Result<Vec<u8>> {
    let result = meteor_asset::runtime_assets::read_file(&path);

    #[cfg(test)]
    if result.is_err() && path.as_ref().is_absolute() {
        return std::fs::read(path);
    }
    result
}
pub(crate) fn read_to_string(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    String::from_utf8(read(path)?)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub(crate) fn required(name: &str) -> Vec<u8> {
    if let Ok(bytes) = read(name) {
        return bytes;
    }
    #[cfg(test)]
    {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
        let packs =
            meteor_asset::pack_repository::PackRepository::open(&root).expect("test MPK fixtures");
        return packs.read(name).expect("test startup asset");
    }
    #[cfg(not(test))]
    panic!("required startup MPK entry missing: {name}")
}
