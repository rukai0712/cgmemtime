use clap::{ArgAction, Parser, Subcommand};
use std::fs::{metadata, read_dir, File};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use tempfile::{Builder, TempDir};

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

    #[arg(skip)]
    temp_cg_dir: Option<TempDir>,

    #[arg(action=ArgAction::SetTrue, short='t', help="machine readable output (delimited columns)")]
    machine_readable: bool,
    #[arg(short = 'd', help = "column delimiter", default_value = ";")]
    delim: char,
    #[arg(action=ArgAction::SetTrue, short='Z', help="disable falling back to systemd-run")]
    disable_systemd_run: bool,

    #[command(subcommand)]
    command: SubCmd,
}

#[derive(Subcommand, Debug)]
enum SubCmd {
    #[command(external_subcommand)]
    Variant(Vec<String>),
}

impl Args {
    fn check_cgroupfs(&mut self) -> &mut Self {
        let dir = Path::new(&self.cg_fs_dir);
        let files = [
            dir.join("cgroup.controllers"),
            dir.join("cgroup.subtree_control"),
        ];
        for file in files {
            let mut buf = String::new();
            File::open(&file)
                .expect(format!("Can't open file: {} ", file.display()).as_str())
                .take(1024)
                .read_to_string(&mut buf)
                .expect(format!("Can't display file: {} ", file.display()).as_str());
            buf.find("memory ")
                .or(buf.find("memory\0"))
                .expect(format!("Cgroup memory controller isn't {}", file.display()).as_str());
        }
        self
    }

    fn check_cgroup_dir(&mut self) -> &mut Self {
        match &self.cg_dir {
            Some(cg_dir) => {
                let meta = metadata(&cg_dir)
                    .expect(format!("Directory {cg_dir} does not exist.").as_str());
                if !meta.is_dir() {
                    panic!("Path {cg_dir} is not a directory.");
                }
                self
            }
            None => {
                let mut buf = String::new();
                File::open("/proc/self/cgroup")
                    .expect(format!("Can't open /proc/self/cgroup").as_str())
                    .take(1024)
                    .read_to_string(&mut buf)
                    .expect("Can't read /proc/self/cgroup");
                let s_pos = buf.find("/").expect("Cgroup does't contain a slash");
                match buf.find(".service") {
                    Some(e_pos) => {
                        let p_dir = buf.get(s_pos..(e_pos + ".service".len())).unwrap();
                        let tmp_dir = Builder::new()
                            .prefix("cgmt-")
                            .rand_bytes(6)
                            .tempdir_in(p_dir)
                            .expect(format!("Can't create tempdir in folder '{p_dir}'").as_str());
                        self.temp_cg_dir = Some(tmp_dir);
                    }
                    None => self.reexec_with_systemd_run(),
                };
                self
            }
        }
    }

    fn reexec_with_systemd_run(&self) {
        if self.disable_systemd_run {
            eprintln!("Couldn't find user@$UID.service cgroup - cf. -c option");
            exit(119)
        }
        let args: Vec<String> = std::env::args().collect();
        let mut systemd = Command::new("systemd-run");
        systemd
            .arg("--user")
            .arg("--scope")
            .arg("--quiet")
            .arg(args[0].as_str())
            .arg("-Z");
        for arg in args.iter().skip(1) {
            systemd.arg(arg);
        }
        let err = systemd.exec();
        eprintln!("{err}");
        exit(118);
    }

    fn setup_cgroup(&mut self) -> &mut Self {
        let cg_dir = if self.temp_cg_dir.is_some() {
            self.temp_cg_dir.as_ref().unwrap().path()
        } else if self.cg_dir.is_some() {
            Path::new(self.cg_dir.as_ref().unwrap())
        } else {
            panic!("Miss cgroup directory");
        };
        read_dir(cg_dir).expect(format!("Can't open directory {}", cg_dir.display()).as_str());

        // otherwise, without the nested setup we can't add a process to the parent cgroup
        // because we also need to write its cgroup.subtree_control file Cgroup v2
        // disallows doing both (yields EBUSY) - cf. https://unix.stackexchange.com/a/713343/1131
        let leaf_dir = cg_dir.join("leaf");
        std::fs::create_dir(&leaf_dir)
            .expect(format!("Can't make directory {}", leaf_dir.display()).as_str());

        let sub_ctl_file = cg_dir.join("cgroup.subtree_control");
        let mut file = File::options()
            .write(true)
            .open(&sub_ctl_file)
            .expect(format!("Can't open file {}", sub_ctl_file.display()).as_str());
        file.write_all("+memory".as_bytes())
            .expect(format!("Write to file {} failed", sub_ctl_file.display()).as_str());
        file.flush()
            .expect(format!("Flush to file {} failed", sub_ctl_file.display()).as_str());

        // TODO: open fd for leaf_dir
        self
    }

    // fn execute(&self) {
    //     let SubCmd::Variant(args) = &self.command;
    //     assert!(args.len() > 0);
    //     let mut sub_command = Command::new(args[0].as_str());
    //     for arg in args.iter().skip(1) {
    //         sub_command.arg(arg);
    //     }
    //     let err = sub_command.exec();
    //     eprintln!("{err}");
    //     exit(127);
    // }
}

fn main() {
    let mut args = Args::parse();

    args.check_cgroupfs().check_cgroup_dir().setup_cgroup();

    // app.execute();

    println!("Disabled systemd run: {args:?}");
}
