use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// https://stackoverflow.com/questions/4842424/list-of-ansi-color-escape-sequences
pub trait Colorize {
    fn red(&self) -> String;
    fn yellow(&self) -> String;
    fn green(&self) -> String;
    fn bold(&self) -> String;
    fn grey(&self) -> String;
    fn cyan(&self) -> String;
    fn reset_fg(&self) -> String;
    fn reset(&self) -> String;
    fn reverse_colors(&self) -> String;
}

impl Colorize for str {
    fn red(&self) -> String {
        format!("\x1b[31m{self}")
    }
    fn yellow(&self) -> String {
        format!("\x1b[33m{self}")
    }
    fn green(&self) -> String {
        format!("\x1b[32m{self}")
    }
    fn bold(&self) -> String {
        format!("\x1b[1m{self}")
    }
    fn grey(&self) -> String {
        format!("\x1b[2m{self}")
    }
    fn cyan(&self) -> String {
        format!("\x1b[36m{self}")
    }
    fn reset_fg(&self) -> String {
        format!("\x1b[39m{self}")
    }
    fn reset(&self) -> String {
        format!("{self}\x1b[0m")
    }
    fn reverse_colors(&self) -> String {
        format!("\x1b[7m{self}")
    }
}

// ---------------------------------------------------------------------------
// status model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Unread,
    Partial,
    Approved,
}

impl Default for Status {
    fn default() -> Self {
        Status::Unread
    }
}

impl Status {
    fn marker(self) -> &'static str {
        match self {
            Status::Unread => "[ ]",
            Status::Partial => "[~]",
            Status::Approved => "[x]",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Status::Unread => "unread",
            Status::Partial => "partial",
            Status::Approved => "approved",
        }
    }
    fn from_name(s: &str) -> Status {
        match s {
            "partial" => Status::Partial,
            "approved" => Status::Approved,
            _ => Status::Unread,
        }
    }
    fn color(self) -> String {
        match self {
            Status::Unread => "".to_string(),           // terminal default
            Status::Partial => "".yellow(),  // yellow
            Status::Approved => "".green(), // green
        }
    }
}

#[derive(Clone, Copy, Default)]
struct FileState {
    status: Status,
    flagged: bool,
}

impl FileState {
    fn is_default(&self) -> bool {
        self.status == Status::Unread && !self.flagged
    }
}

// ---------------------------------------------------------------------------
// persistent state store (TSV: status<TAB>flag<TAB>canonical_path)
// ---------------------------------------------------------------------------

struct StateStore {
    path: PathBuf,
    map: HashMap<String, FileState>,
}

impl StateStore {
    fn load(path: PathBuf) -> StateStore {
        let mut map = HashMap::new();
        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines() {
                let mut it = line.splitn(3, '\t');
                let st = it.next().unwrap_or("");
                let fl = it.next().unwrap_or("");
                let key = it.next().unwrap_or("");
                if key.is_empty() {
                    continue;
                }
                map.insert(
                    key.to_string(),
                    FileState {
                        status: Status::from_name(st),
                        flagged: fl == "1",
                    },
                );
            }
        }
        StateStore { path, map }
    }

    fn get(&self, key: &str) -> FileState {
        self.map.get(key).copied().unwrap_or_default()
    }

    fn set(&mut self, key: &str, fs_state: FileState) {
        if fs_state.is_default() {
            self.map.remove(key);
        } else {
            self.map.insert(key.to_string(), fs_state);
        }
        self.save();
    }

    fn save(&self) {
        let mut out = String::new();
        for (k, v) in &self.map {
            out.push_str(v.status.name());
            out.push('\t');
            out.push(if v.flagged { '1' } else { '0' });
            out.push('\t');
            out.push_str(k);
            out.push('\n');
        }
        let _ = fs::write(&self.path, out);
    }
}

// ---------------------------------------------------------------------------
// directory entries
// ---------------------------------------------------------------------------

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_parent: bool,
    key: String, // canonical-path key for files; empty for dirs
}

fn canonical_key(p: &Path) -> String {
    fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn read_entries(dir: &Path) -> Vec<Entry> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            // Hide only our own artifacts; keep dotfiles like .github visible.
            if name == ".audit-state" || name == ".audit-flags" {
                continue;
            }
            let path = ent.path();
            let is_dir = path.is_dir();
            if is_dir {
                dirs.push(Entry {
                    name,
                    path,
                    is_dir: true,
                    is_parent: false,
                    key: String::new(),
                });
            } else {
                let key = canonical_key(&path);
                files.push(Entry {
                    name,
                    path,
                    is_dir: false,
                    is_parent: false,
                    key,
                });
            }
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut out = Vec::with_capacity(dirs.len() + files.len() + 1);
    if let Some(parent) = dir.parent() {
        out.push(Entry {
            name: "..".to_string(),
            path: parent.to_path_buf(),
            is_dir: true,
            is_parent: true,
            key: String::new(),
        });
    }
    out.append(&mut dirs);
    out.append(&mut files);
    out
}

// ---------------------------------------------------------------------------
// terminal raw mode via stty (no termios crate, no unsafe)
// ---------------------------------------------------------------------------

const RAW_FLAGS: &[&str] = &[
    "-icanon", "-echo", "-isig", "-opost", "min", "1", "time", "0",
];

fn stty(args: &[&str]) -> io::Result<String> {
    let out = Command::new("stty")
        .args(args)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "stty failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct RawMode {
    saved: String,
}

impl RawMode {
    fn enable() -> io::Result<RawMode> {
        let saved = stty(&["-g"])?;
        stty(RAW_FLAGS)?;
        Ok(RawMode { saved })
    }
    fn restore(&self) {
        let args: Vec<&str> = self.saved.split_whitespace().collect();
        if !args.is_empty() {
            let _ = stty(&args);
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.restore();
        // show cursor, reset attrs, leave the prompt on a fresh line
        print!("\x1b[0m\x1b[?25h\r\n");
        let _ = io::stdout().flush();
    }
}

// ---------------------------------------------------------------------------
// launching vim with the audit layer
// ---------------------------------------------------------------------------

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn open_file(file: &Path, flag_dir: &Path, audit_vim: &str, editor: &str, raw: &RawMode) {
    // Hand the terminal back to a sane state so vim can drive it.
    raw.restore();
    print!("\x1b[?25h\x1b[2J\x1b[H");
    let _ = io::stdout().flush();

    let _ = fs::create_dir_all(flag_dir);
    let flagfile = flag_dir.join(format!("{:016x}.flags", fnv1a(&canonical_key(file))));

    let source_cmd = format!("source {}", audit_vim);
    let result = Command::new(editor)
        .arg("-M") // non-modifiable, matching your launch convention
        .arg("-c")
        .arg(&source_cmd)
        .arg(file)
        .env("AUDIT_FLAGFILE", &flagfile)
        .status();

    if let Err(e) = result {
        eprintln!("audit-ui: failed to launch '{}': {}", editor, e);
        eprintln!("press enter to continue");
        let mut s = String::new();
        let _ = io::stdin().read_line(&mut s);
    }

    // Back to our raw mode for the UI.
    let _ = stty(RAW_FLAGS); // TODO why can't I just raw.enable here?
    print!("\x1b[?25l");
    let _ = io::stdout().flush();
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

fn render(cwd: &Path, entries: &[Entry], selected: usize, store: &StateStore, msg: &str) {
    let mut out = String::new();
    out.push_str("\x1b[2J\x1b[H"); // clear + home

    out.push_str(&"audit  ".bold().reset());
    out.push_str(&cwd.display().to_string());
    out.push_str("\r\n\r\n");

    for (i, e) in entries.iter().enumerate() {
        let selected_row = i == selected;
        if selected_row {
            out.push_str(&"".reverse_colors());
        }

        if e.is_dir {
            out.push_str(&"".cyan()); 
            out.push_str("        "); // align under "[x] !  "
            if e.is_parent {
                out.push_str(".. (up)");
            } else {
                out.push_str(&e.name);
                out.push('/');
            }
        } else {
            let st = store.get(&e.key);
            let flag = if st.flagged { &"!".red() } else { " " };
            out.push_str(&st.status.color());
            out.push_str(st.status.marker());
            out.push_str(&" ".reset_fg());
            out.push_str(flag);
            out.push_str(&" ".reset_fg());
            out.push_str(&st.status.color());
            out.push_str(&e.name);
        }

        out.push_str("\x1b[0m\r\n");
    }

    out.push_str("\r\n");
    // TODO grab keybinds from actual keybinds
    out.push_str(
        &"[k/l] move  [enter] open/cd  [h] up  \
         [u]nread [p]artial [a]pproved  [space] cycle  [f]lag  [q]uit\r\n"
            .grey()
            .reset(),
    );
    if !msg.is_empty() {
        out.push_str(msg);
        out.push_str("\r\n");
    }

    let stdout = io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(out.as_bytes());
    let _ = lock.flush();
}

// ---------------------------------------------------------------------------
// input
// ---------------------------------------------------------------------------

enum Action {
    Up,
    Down,
    Top,
    Bottom,
    Activate,
    GoUp,
    SetUnread,
    SetPartial,
    SetApproved,
    Cycle,
    ToggleFlag,
    Quit,
    None,
}

fn parse_key(key: &[u8]) -> Action {
    if key.len() >= 3 && key[0] == 0x1b && key[1] == b'[' {
        return match key[2] {
            b'A' => Action::Up,
            b'B' => Action::Down,
            b'C' => Action::Activate,
            b'D' => Action::GoUp,
            _ => Action::None,
        };
    }
    match key[0] {
        b'l' => Action::Up,
        b'k' => Action::Down,
        b'g' => Action::Top,
        b'G' => Action::Bottom,
        b'l' | b'\r' | b'\n' => Action::Activate,
        b'h' | 0x7f | 0x08 => Action::GoUp,
        b'u' => Action::SetUnread,
        b'p' => Action::SetPartial,
        b'a' => Action::SetApproved,
        b' ' => Action::Cycle,
        b'f' => Action::ToggleFlag,
        b'q' | 0x03 => Action::Quit,
        _ => Action::None,
    }
}

fn mutate_selected(
    store: &mut StateStore,
    entries: &[Entry],
    selected: usize,
    f: impl Fn(&mut FileState),
) {
    if let Some(e) = entries.get(selected) {
        if !e.is_dir {
            let mut s = store.get(&e.key);
            f(&mut s);
            store.set(&e.key, s);
        }
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let mut argv = env::args().skip(1);
    let start_dir = argv
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut cwd = fs::canonicalize(&start_dir).unwrap_or(start_dir);
    if !cwd.is_dir() {
        if let Some(p) = cwd.parent() {
            cwd = p.to_path_buf();
        }
    }

    // TODO why is this different fom cwd?
    let proc_cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let state_path = proc_cwd.join(".audit-state");
    let flag_dir = proc_cwd.join(".audit-flags");

    // TODO We should bring our own vim script. Maybe we can point to the include directory?
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let audit_vim_raw = env::var("AUDIT_VIM").unwrap_or_else(|_| "audit.vim".to_string());
    let audit_vim = fs::canonicalize(&audit_vim_raw)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| audit_vim_raw.clone());
    let audit_vim_missing = fs::metadata(&audit_vim).is_err();

    let mut store = StateStore::load(state_path);
    let mut entries = read_entries(&cwd);
    let mut selected = 0usize;
    let mut msg = if audit_vim_missing {
        format!(
            "warning: audit.vim not found at {} — set AUDIT_VIM",
            audit_vim
        )
        .yellow()
        .reset()
    } else {
        String::new()
    };

    let raw = match RawMode::enable() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("audit-ui: could not enter raw mode: {}", e);
            return;
        }
    };
    print!("\x1b[?25l"); // hide cursor
    let _ = io::stdout().flush();

    let mut stdin = io::stdin();
    let mut buf = [0u8; 8];

    loop {
        if selected >= entries.len() && !entries.is_empty() {
            selected = entries.len() - 1;
        }
        render(&cwd, &entries, selected, &store, &msg);
        msg.clear();

        let n = match stdin.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(_) => break,
        };

        match parse_key(&buf[..n]) {
            Action::Up => selected = selected.saturating_sub(1),
            Action::Down => {
                if selected + 1 < entries.len() {
                    selected += 1;
                }
            }
            Action::Top => selected = 0,
            Action::Bottom => selected = entries.len().saturating_sub(1),
            Action::GoUp => {
                if let Some(parent) = cwd.parent() {
                    cwd = parent.to_path_buf();
                    entries = read_entries(&cwd);
                    selected = 0;
                }
            }
            Action::Activate => {
                let info = entries.get(selected).map(|e| (e.is_dir, e.path.clone()));
                if let Some((is_dir, path)) = info {
                    if is_dir {
                        cwd = fs::canonicalize(&path).unwrap_or(path);
                        entries = read_entries(&cwd);
                        selected = 0;
                    } else {
                        open_file(&path, &flag_dir, &audit_vim, &editor, &raw);
                        let key = &entries[selected].key;
                        let mut st = store.get(key);
                        if st.status == Status::Unread {
                            st.status = Status::Partial;
                            store.set(key, st);
                        }
                    }
                }
            }
            Action::SetUnread => mutate_selected(&mut store, &entries, selected, |s| {
                s.status = Status::Unread
            }),
            Action::SetPartial => mutate_selected(&mut store, &entries, selected, |s| {
                s.status = Status::Partial
            }),
            Action::SetApproved => mutate_selected(&mut store, &entries, selected, |s| {
                s.status = Status::Approved
            }),
            Action::Cycle => mutate_selected(&mut store, &entries, selected, |s| {
                s.status = match s.status {
                    Status::Unread => Status::Partial,
                    Status::Partial => Status::Approved,
                    Status::Approved => Status::Unread,
                };
            }),
            Action::ToggleFlag => {
                mutate_selected(&mut store, &entries, selected, |s| s.flagged = !s.flagged)
            }
            Action::Quit => break,
            Action::None => {}
        }
    }
    // RawMode::drop restores the terminal here.
}
