use clap::{ArgAction, Parser, Subcommand};
use clone3::Clone3;
use nix::fcntl;
use nix::libc;
use nix::sys::signal;
use nix::sys::stat::Mode;
use nix::sys::time::TimeVal;
use std::fs;
use std::fs::{metadata, read_dir, File};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::time::SystemTime;
use tempfile::{Builder, TempDir};

#[derive(Parser, Debug)]
#[command(allow_external_subcommands = true)]
pub struct Args {
    #[arg(short = 'm', help = "Cgroup v2 base", default_value = "/sys/fs/cgroup")]
    cg_fs_dir: String,
    #[arg(short = 'c', help = "Cgroup v2 base")]
    cg_dir: Option<String>,

    #[arg(skip)]
    temp_cg_dir: Option<PathBuf>,
    #[arg(skip)]
    leaf_dir: Option<PathBuf>,

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
                let s_pos = buf.find("/").expect("Cgroup does't contain a slash") + 1;
                match buf.find(".service") {
                    Some(e_pos) => {
                        let p_dir = buf.get(s_pos..(e_pos + ".service".len())).unwrap();
                        let p_dir = Path::new(self.cg_fs_dir.as_str()).join(p_dir);
                        let tmp_dir = Builder::new()
                            .prefix("cgmt-")
                            .rand_bytes(6)
                            .tempdir_in(&p_dir)
                            .expect(
                                format!("Can't create tempdir in folder '{}'", p_dir.display())
                                    .as_str(),
                            )
                            .into_path();
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
            self.temp_cg_dir.as_ref().unwrap().as_path()
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
        self.leaf_dir = Some(leaf_dir);

        let sub_ctl_file = cg_dir.join("cgroup.subtree_control");
        let mut file = File::options()
            .write(true)
            .open(&sub_ctl_file)
            .expect(format!("Can't open file {}", sub_ctl_file.display()).as_str());
        file.write_all("+memory".as_bytes())
            .expect(format!("Write to file {} failed", sub_ctl_file.display()).as_str());
        file.flush()
            .expect(format!("Flush to file {} failed", sub_ctl_file.display()).as_str());

        self
    }

    fn execute(self) {
        let leaf_dir = self.leaf_dir.as_ref().unwrap();

        let fd = fcntl::open(
            leaf_dir,
            fcntl::OFlag::O_RDONLY | fcntl::OFlag::O_DIRECTORY,
            Mode::empty(),
        )
        .unwrap();

        // Dir
        let mut pidfd = -1;
        let mut clone = Clone3::default();
        clone
            .flag_pidfd(&mut pidfd)
            .flag_vfork()
            .exit_signal(signal::SIGCHLD as u64)
            .flag_into_cgroup(&fd);

        let t_start = SystemTime::now();

        match unsafe { clone.call() }.unwrap() {
            0 => {
                // child
                let SubCmd::Variant(args) = &self.command;
                assert!(args.len() > 0);
                let mut sub_command = Command::new(args[0].as_str());
                for arg in args.iter().skip(1) {
                    sub_command.arg(arg);
                }
                let err = sub_command.exec();
                eprintln!("{err}");
                exit(127);
            }
            child_pid => {
                // parent
                // otherwise, Ctrl+C/+] also kill cgmemtime before it has a chance printing its summary
                let sa = signal::SigAction::new(
                    signal::SigHandler::SigIgn,
                    signal::SaFlags::all(),
                    signal::SigSet::empty(),
                );
                unsafe {
                    signal::sigaction(signal::Signal::SIGINT, &sa)
                        .expect("Failed to ignore SIGINT");
                    signal::sigaction(signal::Signal::SIGQUIT, &sa)
                        .expect("failed to ignore SIGQUIT");
                };

                let mut status: i32 = 0;
                let mut usg = std::mem::MaybeUninit::<libc::rusage>::zeroed();
                let usg = unsafe {
                    let r = libc::wait4(child_pid, &mut status, 0, usg.as_mut_ptr());
                    if r < 0 {
                        panic!("waitid failed");
                    }
                    usg.assume_init()
                };

                // resource::Usage::
                let child_user = TimeVal::from(usg.ru_utime);
                let child_sys = TimeVal::from(usg.ru_stime);
                let child_wall = SystemTime::now().duration_since(t_start).unwrap();
                let child_rss_highwater = usg.ru_maxrss * 1024;

                // read cg rss high
                let mut buf = String::new();
                File::open(leaf_dir.join("memory.peak"))
                    .expect("Can't open memory.peak (requires Kernel 5.19 or later)")
                    .take(21)
                    .read_to_string(&mut buf)
                    .expect("Can't read memory.peak");
                let cg_rss_highwater: u64 = buf.trim().parse().unwrap();

                println!("child_user: {}", child_user);
                println!("child_sys: {}", child_sys);
                println!("child_wall: {:?}", child_wall);
                println!("child_rss_highwater: {}", child_rss_highwater);
                println!("cg_rss_highwater: {}", cg_rss_highwater);
            }
        }
    }
}

impl Drop for Args {
    fn drop(&mut self) {
        if let Some(leaf_dir) = self.leaf_dir.take() {
            if let Err(err) = fs::remove_dir(&leaf_dir) {
                eprintln!("Failed to remove {}: {:?}", leaf_dir.display(), err);
            }
        }
        if let Some(temp_cg_dir) = self.temp_cg_dir.take() {
            if let Err(err) = fs::remove_dir(&temp_cg_dir) {
                eprintln!("Failed to remove {}: {:?}", temp_cg_dir.display(), err);
            }
        }
    }
}

fn main() {
    let mut args = Args::parse();
    args.check_cgroupfs().check_cgroup_dir().setup_cgroup();
    args.execute();
}
