//! Each command module owns its matcher, typed grammar, effects and execution.
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
    registry.register::<cp::Cp>()?;
    registry.register_type::<cp::CopyChain>()?;
    registry.register::<mkdir::Mkdir>()?;
    registry.register::<touch::Touch>()?;
    registry.register::<rm::Rm>()?;
    registry.register::<mv::Mv>()?;
    registry.register::<sed::Sed>()?;
    registry.register::<compiler::Compiler>()?;
    registry.register::<coan::Coan>()?;
    registry.register::<gawk::Gawk>()?;
    registry.register_type::<python::Python>()?;
    registry.register::<nm::Nm>()?;
    registry.register::<objdump::Objdump>()?;
    registry.register::<readelf::Readelf>()?;
    registry.register::<size::Size>()?;
    registry.register::<strings::Strings>()?;
    registry.register::<addr2line::Addr2line>()?;
    registry.register::<cxxfilt::Cxxfilt>()?;
    registry.register::<objcopy::Objcopy>()?;
    registry.register::<strip::Strip>()?;
    registry.register::<ar::Ar>()?;
    registry.register::<ranlib::Ranlib>()?;
    registry.register::<assembler::Assembler>()?;
    registry.register::<linker::Linker>()?;
    registry.register::<elfedit::Elfedit>()?;
    registry.register::<dwp::Dwp>()?;
    registry.register::<readobj::Readobj>()?;
    registry.register::<dwarfdump::Dwarfdump>()?;
    registry.register::<symbolizer::Symbolizer>()?;
    Ok(())
}
