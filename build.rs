use std::{fs, path::PathBuf};

fn main() {
    let out_dir: PathBuf = std::env::var("OUT_DIR").unwrap().into();
    let shader_out = out_dir.join("shaders");
    fs::create_dir_all(&shader_out).unwrap();

    let compiler = shaderc::Compiler::new().expect("shaderc compiler");
    let mut opts = shaderc::CompileOptions::new().unwrap();
    opts.set_optimization_level(shaderc::OptimizationLevel::Performance);
    opts.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_1 as u32,
    );

    let shaders = [
        (
            "shaders/chaperone.vert",
            shaderc::ShaderKind::Vertex,
            "chaperone.vert.spv",
        ),
        (
            "shaders/chaperone.frag",
            shaderc::ShaderKind::Fragment,
            "chaperone.frag.spv",
        ),
    ];

    for (src_path, kind, out_name) in &shaders {
        let src =
            fs::read_to_string(src_path).unwrap_or_else(|_| panic!("Could not read {src_path}"));

        let artifact = compiler
            .compile_into_spirv(&src, *kind, src_path, "main", Some(&opts))
            .unwrap_or_else(|e| panic!("Shader compile error in {src_path}: {e}"));

        let out_file = shader_out.join(out_name);
        fs::write(&out_file, artifact.as_binary_u8()).unwrap();

        println!("cargo:rerun-if-changed={src_path}");
    }

    println!("cargo:rerun-if-changed=build.rs");
}
