//! Wraith-Setup.exe: a small window that installs the embedded release archive.

use crate::ui::theme::{self, medium as semibold, Palette};
use crate::ui::widgets as w;
use egui::{CornerRadius, FontId, Frame, Margin, RichText};
use std::sync::{Arc, Mutex};

#[cfg(feature = "setup")]
static PAYLOAD: &[u8] = include_bytes!(env!("WRAITH_PAYLOAD"));
#[cfg(not(feature = "setup"))]
static PAYLOAD: &[u8] = &[];

#[derive(Clone, PartialEq)]
enum Step { Ready, Working(String), Done(std::path::PathBuf), Failed(String) }

pub struct Setup { pal: Palette, autostart: bool, desktop: bool, step: Arc<Mutex<Step>> }

pub fn run() -> eframe::Result<()> {
    if !crate::util::is_admin() { crate::util::relaunch_elevated(""); return Ok(()); }
    crate::store::init();
    let icon = egui::IconData { rgba: crate::ui::icon::rgba(64), width: 64, height: 64 };
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_title(crate::tr!("Установка Wraith", "Install Wraith")).with_inner_size([560.0, 440.0]).with_resizable(false).with_icon(Arc::new(icon)), centered: true, ..Default::default() };
    eframe::run_native(crate::tr!("Установка Wraith", "Install Wraith"), options, Box::new(|cc| {
        theme::install_fonts(&cc.egui_ctx);
        let pal = theme::default_palette(); theme::apply(&cc.egui_ctx, &pal);
        Ok(Box::new(Setup { pal, autostart: true, desktop: true, step: Arc::new(Mutex::new(Step::Ready)) }))
    }))
}

impl eframe::App for Setup {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let p = self.pal; let step = self.step.lock().unwrap().clone();
        egui::CentralPanel::default().frame(Frame::new().fill(p.bg).inner_margin(Margin::same(28))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, CornerRadius::same(16), p.accent_soft);
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "W", FontId::new(26.0, semibold()), p.accent);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Wraith").font(FontId::new(24.0, semibold())));
                    ui.label(RichText::new(crate::trf!("Версия {} · Xray", "Version {} · Xray", env!("CARGO_PKG_VERSION"))).color(p.muted));
                });
            });
            ui.add_space(18.0);
            match &step {
                Step::Ready => {
                    ui.label(RichText::new(crate::trf!("Wraith будет установлен в {}", "Wraith will be installed in {}", crate::install::install_dir().display())).color(p.text));
                    ui.add_space(10.0);
                    w::group(ui, &p, |ui| {
                        if w::switch_row(ui, &p, crate::tr!("Запускать вместе с Windows", "Start with Windows"), crate::tr!("Свёрнутым в трей", "Minimised to the tray"), None, self.autostart) { self.autostart = !self.autostart; }
                        w::divider(ui, &p);
                        if w::switch_row(ui, &p, crate::tr!("Ярлык на рабочем столе", "Desktop shortcut"), "", None, self.desktop) { self.desktop = !self.desktop; }
                    });
                    ui.add_space(16.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::pill(ui, &p, crate::tr!("Установить", "Install"), None, true, 0.0).clicked() {
                            let state = self.step.clone(); let options = crate::install::Options { autostart: self.autostart, desktop: self.desktop }; let ctx = ctx.clone();
                            *state.lock().unwrap() = Step::Working(crate::tr!("Подготовка…", "Preparing…").into());
                            std::thread::spawn(move || {
                                let st = state.clone(); let c2 = ctx.clone();
                                let result = crate::install::install(PAYLOAD, &options, &move |s| { *st.lock().unwrap() = Step::Working(s.into()); c2.request_repaint(); });
                                *state.lock().unwrap() = match result { Ok(exe) => Step::Done(exe), Err(e) => Step::Failed(e) };
                                ctx.request_repaint();
                            });
                        }
                        if w::pill(ui, &p, crate::tr!("Отмена", "Cancel"), None, false, 0.0).clicked() { std::process::exit(0); }
                    });
                }
                Step::Working(text) => {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| { ui.add(egui::Spinner::new().size(32.0).color(p.accent)); ui.add_space(12.0); ui.label(RichText::new(text).color(p.muted)); });
                }
                Step::Done(exe) => {
                    ui.label(RichText::new(crate::tr!("Wraith установлен", "Wraith is installed")).font(FontId::new(18.0, semibold())).color(p.accent));
                    ui.label(RichText::new(crate::tr!("Ярлык — в меню «Пуск». Удалить можно в «Параметры → Приложения».", "The shortcut is in the Start menu. Uninstall from Settings → Apps.")).color(p.muted));
                    ui.add_space(20.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::pill(ui, &p, crate::tr!("Запустить Wraith", "Start Wraith"), None, true, 0.0).clicked() {
                            if !crate::install::run_task(crate::install::TASK) { let _ = std::process::Command::new(exe).spawn(); }
                            std::process::exit(0);
                        }
                        if w::pill(ui, &p, crate::tr!("Закрыть", "Close"), None, false, 0.0).clicked() { std::process::exit(0); }
                    });
                }
                Step::Failed(e) => {
                    Frame::new().fill(theme::alpha(p.bad, 0.16)).corner_radius(CornerRadius::same(14)).inner_margin(Margin::same(12)).show(ui, |ui| { ui.set_width(ui.available_width()); ui.label(RichText::new(crate::trf!("Установка не удалась: {e}", "Installation failed: {e}")).color(p.text)); });
                    ui.add_space(16.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::pill(ui, &p, crate::tr!("Повторить", "Retry"), None, true, 0.0).clicked() { *self.step.lock().unwrap() = Step::Ready; }
                        if w::pill(ui, &p, crate::tr!("Закрыть", "Close"), None, false, 0.0).clicked() { std::process::exit(0); }
                    });
                }
            }
        });
    }
}
