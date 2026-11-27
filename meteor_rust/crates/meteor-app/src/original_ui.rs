pub const ASSETS: &[(&str, &[u8])] = &[
    #[cfg(not(feature = "original-directory-default"))]
    ("fonts/ui.ttf", bevy::text::DEFAULT_FONT_DATA),
    (
        "ui/cinematic-title-reborn-matte.png",
        include_bytes!("../assets/ui/cinematic-title-reborn-matte.png"),
    ),
    (
        "ui/mountain-pass-background.jpg",
        include_bytes!("../assets/ui/mountain-pass-background.jpg"),
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
