use std::sync::OnceLock;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Options {
    pub shadows: bool,
    pub batches: bool,
    pub occlusion: bool,
}

impl Options {
    const DEFAULT: Self = Self {
        shadows: true,
        batches: true,
        occlusion: false,
    };
    const OFF: Self = Self {
        shadows: false,
        batches: false,
        occlusion: false,
    };

    fn parse(features: &str) -> Self {
        let enabled = |name| features.split(',').any(|feature| feature.trim() == name);
        Self {
            shadows: enabled("shadows"),
            batches: enabled("batches"),
            occlusion: enabled("occlusion"),
        }
    }
}

struct RuntimeOptions {
    global: Options,
    sn13_override: Option<Options>,
}

impl RuntimeOptions {
    fn from_overrides(global: Option<&str>, sn13: Option<&str>) -> Self {
        if let Some(global) = global {
            return Self {
                global: Options::parse(global),
                sn13_override: None,
            };
        }
        Self {
            global: Options::DEFAULT,
            sn13_override: sn13.map(Options::parse),
        }
    }

    fn for_stage(&self, stage: &str) -> &Options {
        if stage == "sn13" {
            return self.sn13_override.as_ref().unwrap_or(&self.global);
        }
        &self.global
    }
}

const DESKTOP: bool = !cfg!(any(target_arch = "wasm32", target_os = "android"));

fn runtime_options() -> &'static RuntimeOptions {
    static OPTIONS: OnceLock<RuntimeOptions> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let global = std::env::var("METEOR_RENDER_OPTIMIZATIONS").ok();
        let sn13 = std::env::var("METEOR_SN13_RENDER").ok();
        RuntimeOptions::from_overrides(global.as_deref(), sn13.as_deref())
    })
}

pub(super) fn options(stage: &str) -> &'static Options {
    if DESKTOP {
        runtime_options().for_stage(stage)
    } else {
        &Options::OFF
    }
}

pub(super) fn static_shadows(stage: &str) -> bool {
    !DESKTOP || options(stage).shadows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_defaults_enable_batches_and_shadow_cache_on_any_stage() {
        let defaults = RuntimeOptions::from_overrides(None, None);
        for stage in ["sn06", "sn13", "custom-stage"] {
            assert_eq!(
                defaults.for_stage(stage),
                &Options::parse("batches,shadows")
            );
        }
        let full = RuntimeOptions::from_overrides(Some("batches,shadows,occlusion"), None);
        for stage in ["sn06", "sn13", "custom-stage"] {
            assert!(full.for_stage(stage).occlusion);
        }
    }

    #[test]
    fn global_override_takes_precedence_over_legacy_sn13_override() {
        let legacy = RuntimeOptions::from_overrides(None, Some("batches, shadows, occlusion"));
        assert_eq!(legacy.for_stage("sn06"), &Options::DEFAULT);
        assert_eq!(
            legacy.for_stage("sn13"),
            &Options::parse("batches,shadows,occlusion")
        );
        let disabled = RuntimeOptions::from_overrides(Some("none"), Some("occlusion"));
        assert_eq!(disabled.for_stage("sn06"), &Options::OFF);
        assert_eq!(disabled.for_stage("sn13"), &Options::OFF);
    }
}
