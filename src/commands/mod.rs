//! Concrete jobs own their argument grammars, effects, and execution.
//! This list is composition only; neither scheduler nor graph dispatches by kind.
pub mod addr2line;
pub mod ar;
pub mod assembler;
pub(crate) mod binary_common;
pub mod coan;
pub mod compiler;
pub mod cp;
pub mod cxxfilt;
pub mod dwarfdump;
pub mod dwp;
pub mod elfedit;
pub mod external;
pub(crate) mod fs;
pub mod gawk;
pub mod generate_headers_py;
pub mod linker;
pub mod mkdir;
pub mod mv;
pub mod nm;
pub mod objcopy;
pub mod objdump;
pub mod python;
pub mod ranlib;
pub mod readelf;
pub mod readobj;
pub mod rm;
pub mod sed;
pub mod size;
pub mod strings;
pub mod strip;
pub mod symbolizer;
pub mod touch;

pub(crate) fn register_builtins(registry: &mut crate::registry::Registry) -> anyhow::Result<()> {
    registry.register(&["cp"], |argv, hints| {
        Ok(Some(Box::new(cp::Cp::parse(argv, hints)?)))
    })?;
    registry.register(&["mkdir"], |argv, hints| {
        Ok(Some(Box::new(mkdir::Mkdir::parse(argv, hints)?)))
    })?;
    registry.register(&["touch"], |argv, hints| {
        Ok(Some(Box::new(touch::Touch::parse(argv, hints)?)))
    })?;
    registry.register(&["rm"], |argv, hints| {
        Ok(Some(Box::new(rm::Rm::parse(argv, hints)?)))
    })?;
    registry.register(&["mv"], |argv, hints| {
        Ok(Some(Box::new(mv::Mv::parse(argv, hints)?)))
    })?;
    registry.register(&["sed", "gsed"], |argv, hints| {
        Ok(Some(Box::new(sed::Sed::parse(argv, hints)?)))
    })?;
    registry.register(
        &["cc", "c++", "gcc", "g++", "clang", "clang++"],
        |argv, hints| Ok(Some(Box::new(compiler::Compiler::parse(argv, hints)?))),
    )?;
    registry.register(&["coan"], |argv, hints| {
        Ok(Some(Box::new(coan::Coan::parse(argv, hints)?)))
    })?;
    registry.register(&["gawk", "awk"], |argv, hints| {
        Ok(Some(Box::new(gawk::Gawk::parse(argv, hints)?)))
    })?;
    registry.register(&["nm", "gcc-nm", "llvm-nm"], |argv, hints| {
        Ok(Some(Box::new(nm::Nm::parse(argv, hints)?)))
    })?;
    registry.register(&["objdump", "llvm-objdump"], |argv, hints| {
        Ok(Some(Box::new(objdump::Objdump::parse(argv, hints)?)))
    })?;
    registry.register(&["readelf", "llvm-readelf"], |argv, hints| {
        Ok(Some(Box::new(readelf::Readelf::parse(argv, hints)?)))
    })?;
    registry.register(&["size", "llvm-size"], |argv, hints| {
        Ok(Some(Box::new(size::Size::parse(argv, hints)?)))
    })?;
    registry.register(&["strings", "llvm-strings"], |argv, hints| {
        Ok(Some(Box::new(strings::Strings::parse(argv, hints)?)))
    })?;
    registry.register(&["addr2line", "llvm-addr2line"], |argv, hints| {
        Ok(Some(Box::new(addr2line::Addr2line::parse(argv, hints)?)))
    })?;
    registry.register(&["c++filt", "llvm-cxxfilt"], |argv, hints| {
        Ok(Some(Box::new(cxxfilt::Cxxfilt::parse(argv, hints)?)))
    })?;
    registry.register(&["objcopy", "llvm-objcopy"], |argv, hints| {
        Ok(Some(Box::new(objcopy::Objcopy::parse(argv, hints)?)))
    })?;
    registry.register(&["strip", "llvm-strip"], |argv, hints| {
        Ok(Some(Box::new(strip::Strip::parse(argv, hints)?)))
    })?;
    registry.register(&["ar", "gcc-ar", "llvm-ar"], |argv, hints| {
        Ok(Some(Box::new(ar::Ar::parse(argv, hints)?)))
    })?;
    registry.register(&["ranlib", "gcc-ranlib", "llvm-ranlib"], |argv, hints| {
        Ok(Some(Box::new(ranlib::Ranlib::parse(argv, hints)?)))
    })?;
    registry.register(&["as", "gas", "llvm-as", "llvm-mc"], |argv, hints| {
        Ok(Some(Box::new(assembler::Assembler::parse(argv, hints)?)))
    })?;
    registry.register(
        &[
            "ld", "ld.bfd", "ld.gold", "gold", "lld", "ld.lld", "ld64.lld", "lld-link", "llvm-ld",
        ],
        |argv, hints| Ok(Some(Box::new(linker::Linker::parse(argv, hints)?))),
    )?;
    registry.register(&["elfedit"], |argv, hints| {
        Ok(Some(Box::new(elfedit::Elfedit::parse(argv, hints)?)))
    })?;
    registry.register(&["dwp", "llvm-dwp"], |argv, hints| {
        Ok(Some(Box::new(dwp::Dwp::parse(argv, hints)?)))
    })?;
    registry.register(&["readobj", "llvm-readobj"], |argv, hints| {
        Ok(Some(Box::new(readobj::Readobj::parse(argv, hints)?)))
    })?;
    registry.register(&["dwarfdump", "llvm-dwarfdump"], |argv, hints| {
        Ok(Some(Box::new(dwarfdump::Dwarfdump::parse(argv, hints)?)))
    })?;
    registry.register(&["symbolizer", "llvm-symbolizer"], |argv, hints| {
        Ok(Some(Box::new(symbolizer::Symbolizer::parse(argv, hints)?)))
    })?;
    let mut python = python::Python::new();
    python.register(generate_headers_py::GenerateHeadersPy::parse);
    registry.register(&["python", "python3", "py"], move |argv, hints| {
        python.parse(argv, hints)
    })?;
    // Explicit external utilities are unknown-effect jobs, never a shell fallback.
    registry.register_external(&[
        "cat",
        "echo",
        "printf",
        "true",
        "false",
        "ln",
        "chmod",
        "install",
        "sort",
        "uniq",
        "grep",
        "find",
        "head",
        "tail",
        "wc",
        "diff",
        "cmp",
        "tar",
        "gzip",
        "gunzip",
        "unzip",
        "pkg-config",
        "git",
    ])?;
    Ok(())
}
