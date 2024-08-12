use anyhow::Result;
use vergen_gitcl::{BuildBuilder, Emitter, GitclBuilder, RustcBuilder};

pub fn main() -> Result<()> {
    let build = BuildBuilder::all_build()?;
    let gitcl = GitclBuilder::all_git()?;
    let rustc = RustcBuilder::all_rustc()?;

    Emitter::default()
        .add_instructions(&build)?
        .add_instructions(&gitcl)?
        .add_instructions(&rustc)?
        .emit()?;

    Ok(())
}
