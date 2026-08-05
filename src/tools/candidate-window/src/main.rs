//! Candidate list renderer for kime.
//!
//! This process draws candidates and reports mouse clicks. It deliberately does
//! **not** read the keyboard: the input method owns it. On Wayland an input
//! method holds an exclusive, seat wide `zwp_input_method_keyboard_grab_v2`, so
//! key events never reach this window even when it has focus. kime decodes keys
//! itself and drives this renderer over stdin.
//!
//! See `kime_engine_candidate::client` for the wire format.

use std::{
    collections::BTreeMap,
    io::{self, BufRead, Stdout, Write},
    sync::{
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
};

/// Candidates per page. Must match `kime_engine_backend_hanja::PAGE_SIZE`.
const PAGE_SIZE: usize = 10;

/// A command from the input method.
enum Command {
    Page(usize),
    Highlight(usize),
    Quit,
}

struct CandidateApp {
    stdout: Stdout,
    commands: Receiver<Command>,
    page_index: usize,
    highlight: usize,
    max_page_index: usize,
    candidate_list: Vec<(String, String)>,
}

impl CandidateApp {
    /// Report the chosen candidate by absolute index. An index rather than the
    /// text keeps the reply unambiguous; kime resolves it against the same
    /// dictionary slice it sent us.
    fn select(&mut self, index: usize, ctx: &egui::Context) {
        let _ = writeln!(self.stdout, "{index}");
        let _ = self.stdout.flush();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for CandidateApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Page(page) => self.page_index = page.min(self.max_page_index),
                Command::Highlight(index) => {
                    self.highlight = index.min(self.candidate_list.len().saturating_sub(1));
                    self.page_index = (self.highlight / PAGE_SIZE).min(self.max_page_index);
                }
                Command::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let from = self.page_index * PAGE_SIZE;
            let to = (from + PAGE_SIZE).min(self.candidate_list.len());

            let mut clicked = None;

            for (offset, (key, value)) in self.candidate_list[from..to].iter().enumerate() {
                let index = from + offset;
                // The digit that picks this row, matching kime's key handling:
                // 1-9 then 0 for the tenth.
                let hotkey = (offset + 1) % PAGE_SIZE;

                let selected = index == self.highlight;
                let label = format!("{hotkey}.  {key}    {value}");

                if ui
                    .add_sized(
                        [ui.available_width(), 0.0],
                        egui::Button::selectable(selected, label),
                    )
                    .clicked()
                {
                    clicked = Some(index);
                }
            }

            if let Some(index) = clicked {
                self.select(index, ctx);
            }
        });

        egui::TopBottomPanel::bottom("candidate-footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for page in 0..=self.max_page_index {
                    let label = if page == self.page_index {
                        egui::RichText::new(format!("[{}]", page + 1)).color(egui::Color32::YELLOW)
                    } else {
                        egui::RichText::new(format!("{}", page + 1))
                    };

                    if ui.button(label).clicked() {
                        self.page_index = page;
                    }
                }
            });
        });
    }
}

/// Read the candidate list, terminated by an `end` line.
///
/// The list used to be terminated by EOF, which meant stdin could not stay open
/// for later commands.
fn read_candidate_list(stdin: &mut impl BufRead) -> io::Result<Vec<(String, String)>> {
    let mut candidate_list = Vec::new();
    let mut buf = String::with_capacity(4096);

    loop {
        buf.clear();

        if stdin.read_line(&mut buf)? == 0 {
            break;
        }

        let line = buf.trim_end_matches(['\n', '\r']);

        if line == "end" {
            break;
        }

        match line.split('\t').collect::<Vec<_>>()[..] {
            ["c", key, value] => candidate_list.push((key.to_string(), value.to_string())),
            _ => eprintln!("kime-candidate-window: ignoring malformed line {line:?}"),
        }
    }

    Ok(candidate_list)
}

/// Forward commands from the input method into the UI thread.
fn spawn_command_reader(ctx: egui::Context) -> Receiver<Command> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let stdin = io::stdin();
        let mut lines = stdin.lock().lines();

        while let Some(Ok(line)) = lines.next() {
            let command = match line.trim().split_once(' ') {
                Some(("page", arg)) => arg.parse().ok().map(Command::Page),
                Some(("hl", arg)) => arg.parse().ok().map(Command::Highlight),
                _ if line.trim() == "quit" => Some(Command::Quit),
                _ => None,
            };

            let Some(command) = command else {
                eprintln!("kime-candidate-window: ignoring unknown command {line:?}");
                continue;
            };

            let quit = matches!(command, Command::Quit);

            if tx.send(command).is_err() {
                break;
            }

            ctx.request_repaint();

            if quit {
                return;
            }
        }

        // stdin closed: the input method is gone, so this window is orphaned.
        let _ = tx.send(Command::Quit);
        ctx.request_repaint();
    });

    rx
}

fn main() -> io::Result<()> {
    let stdout = io::stdout();

    let candidate_list = {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        read_candidate_list(&mut stdin)?
    };

    if candidate_list.is_empty() {
        eprintln!("kime-candidate-window: empty candidate list");
        return Ok(());
    }

    let max_page_index = (candidate_list.len().div_ceil(PAGE_SIZE)) - 1;

    eframe::run_native(
        "kime-candidate",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([400.0, 400.0])
                .with_decorations(false)
                .with_window_level(egui::WindowLevel::AlwaysOnTop),
            ..Default::default()
        },
        Box::new(move |cc| {
            let config = kime_engine_core::load_engine_config_from_config_dir().unwrap_or_default();
            let (font_bytes, _index) = config.candidate_font;
            let mut font_data = BTreeMap::<_, Arc<egui::FontData>>::new();
            let mut families = BTreeMap::new();

            font_data.insert(
                "Font".to_string(),
                Arc::new(egui::FontData::from_owned(font_bytes.to_vec())),
            );

            families.insert(egui::FontFamily::Proportional, vec!["Font".to_string()]);
            families.insert(egui::FontFamily::Monospace, vec!["Font".to_string()]);

            cc.egui_ctx.set_fonts(egui::FontDefinitions {
                font_data,
                families,
            });

            let commands = spawn_command_reader(cc.egui_ctx.clone());

            Ok(Box::new(CandidateApp {
                stdout,
                commands,
                page_index: 0,
                highlight: 0,
                max_page_index,
                candidate_list,
            }))
        }),
    )
    .map_err(|e| io::Error::other(e.to_string()))?;

    Ok(())
}
