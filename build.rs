fn main() {
    println!("cargo:rerun-if-changed=packaging/resources/claude-computer-host.rc");
    println!("cargo:rerun-if-changed=packaging/resources/claude-computer-host.exe.manifest");
    embed_resource::compile_for(
        "packaging/resources/claude-computer-host.rc",
        ["claude-computer-host"],
        embed_resource::NONE,
    );
}
