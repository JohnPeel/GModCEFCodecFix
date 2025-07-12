use std::sync::LazyLock;

use bitcode::{Decode, Encode};
use futures::{stream::select, SinkExt, StreamExt};
use iced::advanced::graphics::core::Element;
use iced::widget::{column, container, progress_bar, row, text, Button};
use iced::window::{icon, Position};
use iced::{stream, window, Font, Length, Size, Subscription, Task, Theme};
use iced_term::TerminalView;

use crate::ipc::{self, Recv};

static ICON: LazyLock<icon::Icon> = LazyLock::new(|| {
	use iced::advanced::graphics::image::image_rs::ImageFormat;

	icon::from_file_data(
		include_bytes!("../GModPatchToolLogo.png"),
		Some(ImageFormat::Png),
	)
	.expect("failed to load icon data")
});

#[derive(Clone, Debug, Encode, Decode)]
pub enum IpcRequest {
	SetStatus(String),
	SetProgress(f32),
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum IpcResponse {}

#[derive(Clone, Debug)]
pub enum Event {
	Terminal(iced_term::Event),
	IpcError(String),
	Ipc(IpcRequest),
	Toggle,
}

struct App {
	title: String,
	term: iced_term::Terminal,
	status: String,
	progress: f32,
	expanded: bool,
}

impl App {
	fn new() -> (Self, Task<Event>) {
		let program = if let Ok(current_exe) = std::env::current_exe() {
			current_exe.display().to_string()
		} else if cfg!(target_os = "windows") {
			"gmodpatchtool.exe".to_owned()
		} else {
			"gmodpatchtool".to_owned()
		};
		let mut args = std::env::args().skip(1).collect::<Vec<String>>();
		args.push("--enable-ipc".to_owned());

		let term_id = 0;
		let term_settings = iced_term::settings::Settings {
			font: iced_term::settings::FontSettings {
				size: 14.0,
				font_type: Font::MONOSPACE,
				..Default::default()
			},
			theme: iced_term::settings::ThemeSettings::new(Box::new(iced_term::ColorPalette {
				background: "#0c0c0c".to_owned(),
				foreground: "#b4b4b4".to_owned(),
				black: "#000000".to_owned(),
				red: "#c23621".to_owned(),
				green: "#25bc24".to_owned(),
				yellow: "#999900".to_owned(),
				blue: "#0000b2".to_owned(),
				magenta: "#b200b2".to_owned(),
				cyan: "#00a6b2".to_owned(),
				white: "#bfbfbf".to_owned(),
				bright_black: "#666666".to_owned(),
				bright_red: "#e60000".to_owned(),
				bright_green: "#00d900".to_owned(),
				bright_yellow: "#e6e600".to_owned(),
				bright_blue: "#0000ff".to_owned(),
				bright_magenta: "#e600e6".to_owned(),
				bright_cyan: "#00e6e6".to_owned(),
				bright_white: "#e6e6e6".to_owned(),
				..Default::default()
			})),
			backend: iced_term::settings::BackendSettings { program, args },
		};

		(
			Self {
				title: String::from("GModPatchTool"),
				term: iced_term::Terminal::new(term_id, term_settings),
				status: "Starting...".to_owned(),
				progress: 0.0,
				expanded: false,
			},
			Task::none(),
		)
	}

	fn title(&self) -> String {
		self.title.clone()
	}

	fn subscription(&self) -> Subscription<Event> {
		let term_subscription = iced_term::Subscription::new(self.term.id);
		let term_event_stream = term_subscription.event_stream().map(Event::Terminal);

		let ipc_event_stream = stream::channel(1024, |mut tx| async move {
			let mut server = match ipc::listen::<IpcRequest, IpcResponse>().await {
				Ok(server) => server,
				Err(error) => {
					let _ = tx.send(Event::IpcError(error.to_string())).await;
					return;
				}
			};

			loop {
				let request = match server.recv().await {
					Ok(Some(request)) => request,
					// If this is `None`, it means the client side disconnected.
					Ok(None) => break,
					Err(error) => {
						if tx.send(Event::IpcError(error.to_string())).await.is_err() {
							break;
						}
						continue;
					}
				};

				if tx.send(Event::Ipc(request)).await.is_err() {
					// If this is an error, it means the GUI subscription dropped.
					break;
				}
			}
		});

		Subscription::run_with_id(self.term.id, select(term_event_stream, ipc_event_stream))
	}

	fn update(&mut self, event: Event) -> Task<Event> {
		match event {
			Event::Toggle => {
				self.expanded = !self.expanded;
				Task::none()
			}
			Event::IpcError(error) => {
				self.status = format!("IPC ERROR: {error}");
				Task::none()
			}
			Event::Ipc(IpcRequest::SetStatus(status)) => {
				self.status = status;
				Task::none()
			}
			Event::Ipc(IpcRequest::SetProgress(progress)) => {
				self.progress = progress;
				Task::none()
			}
			Event::Terminal(iced_term::Event::CommandReceived(_, cmd)) => {
				let is_init_task = matches!(cmd, iced_term::Command::InitBackend(_));

				let task = match self.term.update(cmd) {
					iced_term::actions::Action::Shutdown => {
						window::get_latest().and_then(window::close)
					}
					iced_term::actions::Action::ChangeTitle(title) => {
						self.title = title;
						Task::none()
					}
					_ => Task::none(),
				};

				// BUG/HACK: Address race condition with InitBackend/ProcessBackendCommand(Resize) and layout_width/num_cols limiting the terminal size
				// TODO: Report to iced_term
				if is_init_task {
					task.chain(Task::done(Event::Terminal(
						iced_term::Event::CommandReceived(
							self.term.id,
							iced_term::Command::ChangeFont(iced_term::settings::FontSettings {
								size: 14.0,
								font_type: Font::MONOSPACE,
								..Default::default()
							}),
						),
					)))
				} else {
					task
				}
			}
		}
	}

	fn view(&self) -> Element<Event, Theme, iced::Renderer> {
		let button = row![
			Button::new(if self.expanded { "v" } else { ">" }).on_press(Event::Toggle),
			"Details",
		];

		let details: Element<Event, Theme, iced::Renderer> = if self.expanded {
			column![
				button,
				container(TerminalView::show(&self.term).map(Event::Terminal))
					.width(Length::Fill)
					.height(Length::Fill)
			]
			.into()
		} else {
			button.into()
		};

		column![
			text(self.status.clone()),
			progress_bar(0.0..=100.0, self.progress),
			details,
		]
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(5)
		.spacing(5)
		.into()
	}
}

pub fn main() -> iced::Result {
	iced::application(App::title, App::update, App::view)
		.antialiasing(true)
		.window(iced::window::Settings {
			size: Size {
				width: 960.0,
				height: 540.0,
			},
			position: Position::Centered,
			resizable: false,
			icon: Some(ICON.clone()),
			..Default::default()
		})
		.subscription(App::subscription)
		.run_with(App::new)
}
