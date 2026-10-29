use bevy_app::{Plugin, PreUpdate};
use bevy_diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use bevy_ecs::{resource::Resource, system::Res};
use bevy_platform::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::{mesh::allocator::MeshAllocator, Extract, ExtractSchedule, RenderApp};

static MESH_ALLOCATOR_SLABS: DiagnosticPath = DiagnosticPath::const_new("mesh_allocator_slabs");

static MESH_ALLOCATOR_SLABS_SIZE: DiagnosticPath =
    DiagnosticPath::const_new("mesh_allocator_slabs_size");

static MESH_ALLOCATOR_ALLOCATIONS: DiagnosticPath =
    DiagnosticPath::const_new("mesh_allocator_allocations");

pub struct MeshAllocatorDiagnosticPlugin;

impl MeshAllocatorDiagnosticPlugin {

    pub fn slabs_diagnostic_path() -> &'static DiagnosticPath {
        &MESH_ALLOCATOR_SLABS
    }

    pub fn slabs_size_diagnostic_path() -> &'static DiagnosticPath {
        &MESH_ALLOCATOR_SLABS_SIZE
    }

    pub fn allocations_diagnostic_path() -> &'static DiagnosticPath {
        &MESH_ALLOCATOR_ALLOCATIONS
    }
}

impl Plugin for MeshAllocatorDiagnosticPlugin {
    fn build(&self, app: &mut bevy_app::App) {
        app.register_diagnostic(
            Diagnostic::new(MESH_ALLOCATOR_SLABS.clone()).with_suffix(" slabs"),
        )
        .register_diagnostic(
            Diagnostic::new(MESH_ALLOCATOR_SLABS_SIZE.clone()).with_suffix(" bytes"),
        )
        .register_diagnostic(
            Diagnostic::new(MESH_ALLOCATOR_ALLOCATIONS.clone()).with_suffix(" meshes"),
        )
        .init_resource::<MeshAllocatorMeasurements>()
        .add_systems(PreUpdate, add_mesh_allocator_measurement);

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(ExtractSchedule, measure_allocator);
        }
    }
}

#[derive(Debug, Default, Resource)]
struct MeshAllocatorMeasurements {
    slabs: AtomicUsize,
    slabs_size: AtomicU64,
    allocations: AtomicUsize,
}

fn add_mesh_allocator_measurement(
    mut diagnostics: Diagnostics,
    measurements: Res<MeshAllocatorMeasurements>,
) {
    diagnostics.add_measurement(&MESH_ALLOCATOR_SLABS, || {
        measurements.slabs.load(Ordering::Relaxed) as f64
    });
    diagnostics.add_measurement(&MESH_ALLOCATOR_SLABS_SIZE, || {
        measurements.slabs_size.load(Ordering::Relaxed) as f64
    });
    diagnostics.add_measurement(&MESH_ALLOCATOR_ALLOCATIONS, || {
        measurements.allocations.load(Ordering::Relaxed) as f64
    });
}

fn measure_allocator(
    measurements: Extract<Res<MeshAllocatorMeasurements>>,
    allocator: Res<MeshAllocator>,
) {
    measurements
        .slabs
        .store(allocator.slab_count(), Ordering::Relaxed);
    measurements
        .slabs_size
        .store(allocator.slabs_size(), Ordering::Relaxed);
    measurements
        .allocations
        .store(allocator.index_allocation_count(), Ordering::Relaxed);
}
