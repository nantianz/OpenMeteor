pub const ASSETS: &[(&str, &[u8])] = &[
    #[cfg(not(feature = "original-directory-default"))]
    ("fonts/ui.ttf", bevy::text::DEFAULT_FONT_DATA),
    (
        "ui/cinematic-title-reborn-matte.png",
        include_bytes!("../assets/ui/cinematic-title-reborn-matte.png"),
    ),
    (
        "ui/cinematic-fortress.png",
        include_bytes!("../assets/ui/cinematic-fortress.png"),
    ),
    (
        "ui/campaign-map.png",
        include_bytes!("../assets/ui/campaign-map.png"),
    ),
    (
        "ui/campaign-halo.png",
        include_bytes!("../assets/ui/campaign-halo.png"),
    ),
    (
        "ui/studio-logo.png",
        include_bytes!("../assets/ui/studio-logo.png"),
    ),
];
