const PLACEHOLDER: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 11, 73, 68, 65, 84, 120, 156, 99, 96, 0, 2, 0, 0, 5, 0, 1,
    122, 94, 171, 63, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

pub const ASSETS: &[(&str, &[u8])] = &[
    ("fonts/ui.ttf", bevy::text::DEFAULT_FONT_DATA),
    ("ui/cinematic-title-reborn-matte.png", PLACEHOLDER),
    ("ui/cinematic-fortress.png", PLACEHOLDER),
    ("ui/campaign-map.png", PLACEHOLDER),
    ("ui/campaign-halo.png", PLACEHOLDER),
    ("ui/studio-logo.png", PLACEHOLDER),
];
