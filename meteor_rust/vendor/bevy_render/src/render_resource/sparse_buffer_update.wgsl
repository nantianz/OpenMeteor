

struct SparseBufferUpdateMetadata {

    element_size: u32,

    updated_page_count: u32,

    page_size_log2: u32,
};

@group(0) @binding(0) var<storage, read_write> dest_buffer: array<u32>;

@group(0) @binding(1) var<storage> src_buffer: array<u32>;

@group(0) @binding(2) var<storage> indices: array<u32>;

@group(0) @binding(3) var<uniform> metadata: SparseBufferUpdateMetadata;

@workgroup_size(256, 1, 1)
@compute
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {

    let invocation_index = global_id.x;
    let total_word_count = (metadata.updated_page_count << metadata.page_size_log2) *
        metadata.element_size;
    if (invocation_index >= total_word_count) {
        return;
    }

    let element_index = invocation_index / metadata.element_size;

    let word_index = invocation_index % metadata.element_size;

    let update_index = element_index >> metadata.page_size_log2;

    let element_index_in_page = element_index & ((1u << metadata.page_size_log2) - 1u);

    let page_index = indices[update_index];

    let dest_index = ((page_index << metadata.page_size_log2) + element_index_in_page) *
        metadata.element_size + word_index;
    if (dest_index >= arrayLength(&dest_buffer)) {
        return;
    }

    let src_index = element_index * metadata.element_size + word_index;
    dest_buffer[dest_index] = src_buffer[src_index];
}
