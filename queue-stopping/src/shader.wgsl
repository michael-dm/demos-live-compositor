struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) VertexIndex: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0)
    );
    var tex_coords = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0)
    );
    
    var output: VertexOutput;
    output.position = vec4<f32>(positions[VertexIndex], 0.0, 1.0);
    output.tex_coords = tex_coords[VertexIndex];
    return output;
}

@group(0) @binding(0) var myTexture: texture_2d<f32>;
@group(0) @binding(1) var mySampler: sampler;

@fragment
fn fs_main(@location(0) tex_coords: vec2<f32>) -> @location(0) vec4<f32> {
    let color = textureSample(myTexture, mySampler, tex_coords);
    
    // Gamma correction (assuming the input is in sRGB space)
    let gamma = 2.2;
    let corrected_color = pow(color.rgb, vec3<f32>(gamma));
    
    return vec4<f32>(corrected_color, color.a);
}