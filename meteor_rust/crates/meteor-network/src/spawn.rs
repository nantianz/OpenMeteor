use meteor_asset::des::{StageDescription, StageObject};
use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnPad {
    pub group: u32,
    pub index: u32,
}

pub fn group(camp: u32) -> u32 {
    match camp {
        1 | 2 => camp,
        _ => 0,
    }
}

pub fn pads(desc: &StageDescription, camp: u32) -> impl Iterator<Item = &StageObject> {
    let prefix = match group(camp) {
        1 => "teama",
        2 => "teamb",
        _ => "user",
    };
    desc.dummies.iter().filter(move |object| {
        object
            .name_attr
            .as_deref()
            .is_some_and(|name| name.to_ascii_lowercase().starts_with(prefix))
    })
}

pub fn counts(desc: &StageDescription) -> [usize; 3] {
    std::array::from_fn(|group| pads(desc, group as u32).count())
}

pub fn random_index(count: usize) -> Option<usize> {
    (count > 0).then(|| rand::thread_rng().gen_range(0..count))
}
