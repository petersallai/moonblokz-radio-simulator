use eframe::egui;
use embassy_executor::{Executor, Spawner};
use embassy_time::Timer;
use env_logger::Builder;
use log::{LevelFilter, debug, info};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
enum Msg {
    Tick,
}

#[derive(Default)]
struct AppState {
    counter: i32,
    rx: Option<Receiver<Msg>>,
}

impl AppState {
    fn new(rx: Receiver<Msg>) -> Self {
        Self { counter: 0, rx: Some(rx) }
    }
}

impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Repaint periodically so background updates are visible without input
        ctx.request_repaint_after(Duration::from_millis(100));

        // Drain background messages non-blockingly
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::Tick => {
                        self.counter += 1;
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading(format!("Counter: {}", self.counter));
                if ui.button("Increment").clicked() {
                    self.counter += 1;
                    debug!("counter: {}", self.counter);
                }
            });
        });
    }
}

#[embassy_executor::task]
async fn ticker_task(tx: Sender<Msg>) {
    loop {
        let _ = tx.send(Msg::Tick);
        // Sleep using Embassy time on the executor
        Timer::after_millis(1000).await;
    }
}

fn embassy_init(spawner: Spawner, tx: Sender<Msg>) {
    let _ = spawner.spawn(ticker_task(tx));
}

fn main() {
    // Logging setup
    Builder::new()
        .filter_level(LevelFilter::Info)
        .filter(Some("moonblokz_radio_simulator"), LevelFilter::Debug)
        .filter(Some("moonblokz_radio_lib"), LevelFilter::Debug)
        .init();

    info!("Starting up");

    // Channel for background -> UI messages
    let (tx, rx) = channel::<Msg>();

    // Spawn Embassy executor on a dedicated background thread
    let _embassy_handle = thread::Builder::new()
        .name("embassy-executor".to_string())
        .spawn(move || {
            // Leak the executor to satisfy the 'static lifetime required by run()
            let executor: &'static mut Executor = Box::leak(Box::new(Executor::new()));
            executor.run(|spawner| embassy_init(spawner, tx.clone()));
        })
        .expect("failed to spawn embassy thread");

    // Start the GUI on the main thread (required on macOS)
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default(),
        ..Default::default()
    };
    let _ = eframe::run_native("MoonBlokz Radio Simulator", native_options, Box::new(move |_cc| Box::new(AppState::new(rx))));
}
