use clap::{ArgAction, Parser, Subcommand};
use std::{
    ffi::OsString,
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

fn read_file_string(path: &str) -> Result<String, std::io::Error> {
    let mut contents = String::new();
    File::open(path)?.take(1024).read_to_string(&mut contents)?;
    Ok(contents)
}

#[derive(Parser, Debug)]
#[command(allow_external_subcommands = true)]
pub struct Args {
    #[arg(short = 'm', help = "Cgroup v2 base", default_value = "/sys/fs/cgroup")]
    cg_fs_dir: String,
    #[arg(short = 'c', help = "Cgroup v2 base")]
    cg_dir: Option<String>,
    #[arg(action=ArgAction::SetTrue, short='t', help="machine readable output (delimited columns)")]
    machine_readable: bool,
    #[arg(short = 'd', help = "column delimiter", default_value = ";")]
    delim: char,
    #[arg(action=ArgAction::SetTrue, short='Z', help="disable falling back to systemd-run")]
    disable_systemd_run: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(external_subcommand)]
    Variant(Vec<String>),
}

fn main() {
    let args = Args::parse();

    println!("Disabled systemd run: {args:?}");
}
