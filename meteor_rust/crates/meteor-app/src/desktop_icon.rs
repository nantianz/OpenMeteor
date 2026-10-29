use bevy::ecs::system::NonSendMarker;

pub(crate) fn install(_main_thread: NonSendMarker) {}
