use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use ruffle_frontend_utils::backends::navigator::NavigatorInterface;
use ruffle_frontend_utils::content::ContentDescriptor;
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use url::Url;
use winit::event_loop::EventLoopProxy;

use ruffle_core::backend::navigator::{
    ErrorResponse, NavigationMethod, NavigatorBackend, OwnedFuture, Request, SuccessResponse,
};
use ruffle_core::loader::Error as NavError;
use ruffle_frontend_utils::backends::navigator::{ExternalNavigatorBackend, FutureSpawner};
use std::borrow::Cow;
use ba2::Reader;

use crate::cli::{FilesystemAccessMode, OpenUrlMode};
use crate::custom_event::RuffleEvent;
use crate::gui::DialogDescriptor;
use crate::gui::dialogs::filesystem_access_dialog::{
    FilesystemAccessDialogConfiguration, FilesystemAccessDialogResult,
};
use crate::gui::dialogs::network_access_dialog::{
    NetworkAccessDialogConfiguration, NetworkAccessDialogResult,
};
use crate::preferences::GlobalPreferences;
use crate::util::open_url;

// TODO Make this more generic, maybe a manager?
#[derive(Clone)]
pub struct PathAllowList {
    allowed_path_prefixes: Arc<Mutex<Vec<PathBuf>>>,
}

impl PathAllowList {
    pub fn new(content_descriptor: &ContentDescriptor) -> Self {
        let mut allowed_path_prefixes = Vec::new();

        if let Ok(movie_path) = content_descriptor.url.to_file_path() {
            if let Some(parent) = movie_path.parent() {
                // TODO Remove it after integrating recents & bookmarks with
                //   opening a directory.
                allowed_path_prefixes.push(parent.to_path_buf());
            }
            allowed_path_prefixes.push(movie_path);
        }

        if let Some(root_content_path) = &content_descriptor.root_content_path {
            allowed_path_prefixes.push(root_content_path.clone());
        }

        Self {
            allowed_path_prefixes: Arc::new(Mutex::new(allowed_path_prefixes)),
        }
    }

    /// Checks whether the user already allowed access to the file.
    pub fn is_path_allowed(&self, path: &Path) -> bool {
        for path_prefix in self
            .allowed_path_prefixes
            .lock()
            .expect("Non-poisoned lock")
            .as_slice()
        {
            let Ok(path_prefix) = path_prefix.canonicalize() else {
                continue;
            };
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if path.starts_with(path_prefix) {
                return true;
            }
        }
        false
    }

    pub fn add_allowed_path_prefix(&self, path_prefix: PathBuf) {
        self.allowed_path_prefixes
            .lock()
            .expect("Non-poisoned lock")
            .push(path_prefix);
    }
}

#[derive(Clone)]
pub struct DesktopNavigatorInterface {
    preferences: GlobalPreferences,

    // Arc + Mutex due to macOS
    event_loop: Arc<Mutex<EventLoopProxy<RuffleEvent>>>,

    filesystem_access_mode: FilesystemAccessMode,

    allow_list: PathAllowList,
}

impl DesktopNavigatorInterface {
    pub fn new(
        preferences: GlobalPreferences,
        event_loop: EventLoopProxy<RuffleEvent>,
        initial_allow_list: PathAllowList,
        filesystem_access_mode: FilesystemAccessMode,
    ) -> Self {
        Self {
            preferences,
            event_loop: Arc::new(Mutex::new(event_loop)),
            allow_list: initial_allow_list,
            filesystem_access_mode,
        }
    }

    async fn ask_for_filesystem_access(&self, path: &Path) -> bool {
        let (notifier, receiver) = oneshot::channel();
        let _ = self
            .event_loop
            .lock()
            .expect("Non-poisoned event loop")
            .send_event(RuffleEvent::OpenDialog(DialogDescriptor::FilesystemAccess(
                FilesystemAccessDialogConfiguration::new(
                    notifier,
                    self.allow_list.clone(),
                    path.to_path_buf(),
                ),
            )));

        receiver.await == Ok(FilesystemAccessDialogResult::Allow)
    }
}

impl NavigatorInterface for DesktopNavigatorInterface {
    fn navigate_to_website(&self, url: Url) {
        let open_url_mode = self.preferences.open_url_mode();
        if open_url_mode == OpenUrlMode::Deny {
            tracing::warn!("SWF tried to open a website, but opening a website is not allowed");
            return;
        }

        if open_url_mode == OpenUrlMode::Allow {
            open_url(&url);
            return;
        }

        let _ = self
            .event_loop
            .lock()
            .expect("Non-poisoned event loop")
            .send_event(RuffleEvent::OpenDialog(DialogDescriptor::OpenUrl(url)));
    }

    async fn open_file(&self, path: &Path) -> io::Result<File> {
        let path = &path.canonicalize()?;

        let allow = if self.allow_list.is_path_allowed(path) {
            true
        } else {
            match self.filesystem_access_mode {
                FilesystemAccessMode::Allow => true,
                FilesystemAccessMode::Deny => false,
                FilesystemAccessMode::Ask => self.ask_for_filesystem_access(path).await,
            }
        };

        if !allow {
            return Err(ErrorKind::PermissionDenied.into());
        }

        File::open(path).or_else(|e| {
            if cfg!(feature = "sandbox") {
                use rfd::FileDialog;
                let parent_path = path.parent().unwrap_or(path);

                if e.kind() == ErrorKind::PermissionDenied {
                    let attempt_sandbox_open = MessageDialog::new()
                        .set_level(MessageLevel::Warning)
                        .set_description(format!("The current movie is attempting to read files stored in {parent_path:?}.\n\nTo allow it to do so, click Yes, and then Open to grant read access to that directory.\n\nOtherwise, click No to deny access."))
                        .set_buttons(MessageButtons::YesNo)
                        .show() == MessageDialogResult::Yes;

                    if attempt_sandbox_open {
                        FileDialog::new().set_directory(parent_path).pick_folder();

                        return File::open(path);
                    }
                }
            }

            Err(e)
        })
    }

    async fn confirm_socket(&self, host: &str, port: u16) -> bool {
        let (notifier, receiver) = oneshot::channel();
        let _ = self
            .event_loop
            .lock()
            .expect("Non-poisoned event loop")
            .send_event(RuffleEvent::OpenDialog(DialogDescriptor::NetworkAccess(
                NetworkAccessDialogConfiguration::new(notifier, host, port),
            )));
        let result = receiver.await;
        result == Ok(NetworkAccessDialogResult::Allow)
    }
}

struct MemoryResponse {
    url: String,
    body: Vec<u8>,
}

impl SuccessResponse for MemoryResponse {
    fn url(&self) -> Cow<'_, str> { Cow::Borrowed(&self.url) }
    fn set_url(&mut self, url: String) { self.url = url; }
    fn body(self: Box<Self>) -> OwnedFuture<Vec<u8>, NavError> {
        Box::pin(async move { Ok(self.body) })
    }
    fn text_encoding(&self) -> Option<&'static encoding_rs::Encoding> { None }
    fn status(&self) -> u16 { 200 }
    fn redirected(&self) -> bool { false }
    fn next_chunk(&mut self) -> OwnedFuture<Option<Vec<u8>>, NavError> {
        let data = std::mem::take(&mut self.body);
        Box::pin(async move { Ok(if data.is_empty() { None } else { Some(data) }) })
    }
    fn expected_length(&self) -> Result<Option<u64>, NavError> {
        Ok(Some(self.body.len() as u64))
    }
}

/// A custom wrapper around Ruffle's ExternalNavigatorBackend to intercept
/// requests for Fallout 76 Holotape .pip binary save files and redirect
/// them to our local JSON parser.
pub struct HolotapeNavigatorBackend<F: FutureSpawner<NavError>, I: NavigatorInterface> {
    inner: ExternalNavigatorBackend<F, I>,
}

impl<F: FutureSpawner<NavError>, I: NavigatorInterface> HolotapeNavigatorBackend<F, I> {
    pub fn new(inner: ExternalNavigatorBackend<F, I>) -> Self {
        Self { inner }
    }
}

impl<F: FutureSpawner<NavError> + 'static, I: NavigatorInterface + 'static> NavigatorBackend for HolotapeNavigatorBackend<F, I> {
    fn navigate_to_url(
        &self,
        url: &str,
        target: &str,
        vars_method: Option<(NavigationMethod, ruffle_core::indexmap::IndexMap<String, String>)>,
    ) {
        self.inner.navigate_to_url(url, target, vars_method)
    }

    fn fetch(&self, request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        let url_str = request.url().to_string();

        // 1. Intercept BA2 archive streaming requests
        if url_str.starts_with("ba2://") {
            let target_file = url_str["ba2://".len()..].trim_start_matches('/').replace('/', "\\");

            return Box::pin(async move {
                tracing::info!("Extracting {} from SeventySix - Interface.ba2", target_file);

                let mut swf_data = vec![];
                // Open the Bethesda Archive
                if let Ok(mut file) = std::fs::File::open("SeventySix - Interface.ba2") {
                    // Read the archive using the `ba2` crate
                    if let Ok((archive, _)) = ba2::fo4::Archive::read(&file) {
                        // Hash the string path using ba2's specific hashing algorithm.
                        // .into() converts the string bytes to bstr::BStr required by ba2.
                        let hash: ba2::fo4::FileHash = ba2::fo4::hash_file(target_file.as_bytes().into()).0;
                        if let Some(entry) = archive.get(&hash) {
                            swf_data = match entry {
                                ba2::fo4::File::General(f) => f.read_contents(&mut file).unwrap_or_default(),
                                _ => vec![],
                            };
                            tracing::info!("Successfully extracted {} bytes", swf_data.len());
                        } else {
                            tracing::error!("File not found in BA2: {}", target_file);
                        }
                    }
                }

                if swf_data.is_empty() {
                    return Err(ErrorResponse {
                        url: url_str.clone(),
                        error: NavError::FetchError(format!("Failed to extract {}", target_file)),
                    });
                }

                let response: Box<dyn SuccessResponse> = Box::new(MemoryResponse {
                    url: url_str,
                    body: swf_data,
                });
                Ok(response)
            });
        }

        // 2. Intercept local High Score .pip saves
        if url_str.ends_with("HolotapeGameData.pip") {
            let is_post = matches!(request.method(), NavigationMethod::Post);
            let body = request.body().clone().map(|(data, _)| data).unwrap_or_default();

            return Box::pin(async move {
                if is_post {
                    // Intercepting a save request from the SWF
                    tracing::info!("Intercepted save to HolotapeGameData.pip");
                    if let Ok(json) = crate::pip_parser::parse_pip_to_json(&body) {
                        let _ = std::fs::create_dir_all("saves");
                        let _ = std::fs::write("saves/savedata.json", json);
                    }
                    let response: Box<dyn SuccessResponse> = Box::new(MemoryResponse {
                        url: url_str,
                        body: vec![],
                    });
                    Ok(response)
                } else {
                    // Intercepting a load request from the SWF
                    tracing::info!("Intercepted load from HolotapeGameData.pip");
                    let pip_bytes = if let Ok(json) = std::fs::read_to_string("saves/savedata.json") {
                        crate::pip_parser::parse_json_to_pip(&json).unwrap_or_default()
                    } else {
                        vec![] // Return default/empty state if no save exists
                    };

                    let response: Box<dyn SuccessResponse> = Box::new(MemoryResponse {
                        url: url_str,
                        body: pip_bytes,
                    });
                    Ok(response)
                }
            });
        }

        self.inner.fetch(request)
    }

    fn resolve_url(&self, url: &str) -> Result<Url, url::ParseError> {
        if url.starts_with("ba2://") {
            Url::parse(url)
        } else {
            self.inner.resolve_url(url)
        }
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), NavError>) {
        self.inner.spawn_future(future)
    }

    fn pre_process_url(&self, url: Url) -> Url {
        self.inner.pre_process_url(url)
    }

    fn connect_socket(
        &mut self,
        host: String,
        port: u16,
        timeout: std::time::Duration,
        handle: ruffle_core::socket::SocketHandle,
        receiver: async_channel::Receiver<Vec<u8>>,
        sender: async_channel::Sender<ruffle_core::socket::SocketAction>,
    ) {
        self.inner.connect_socket(host, port, timeout, handle, receiver, sender)
    }
}
