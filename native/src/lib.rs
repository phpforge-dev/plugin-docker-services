mod policy;
use crate::policy::{confined, err, Res};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use tokio::{io::AsyncReadExt, process::Command, sync::Mutex};

pub struct Docker {
    program: String,
    paused: AtomicBool,
    mutation: Mutex<()>,
    operations: StdMutex<HashMap<String, Arc<tokio::sync::Notify>>>,
}
struct Operation<'a> {
    owner: &'a Docker,
    id: String,
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if let Ok(mut ops) = self.owner.operations.lock() {
            ops.remove(&self.id);
        }
    }
}
impl Default for Docker {
    fn default() -> Self {
        Self {
            program: if std::env::var_os("PHP_IDE_NATIVE_SMOKE").is_some() {
                std::env::var("PHP_IDE_DOCKER_BIN").unwrap_or_else(|_| "docker".into())
            } else {
                "docker".into()
            },
            paused: AtomicBool::new(false),
            mutation: Mutex::new(()),
            operations: StdMutex::new(HashMap::new()),
        }
    }
}
pub fn container_id(id: &str) -> Res<()> {
    if !(12..=64).contains(&id.len()) || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Identificador de container inválido".into());
    }
    Ok(())
}
fn image_reference(reference: &str) -> Res<()> {
    if reference.len() > 255
        || reference.is_empty()
        || !reference.as_bytes()[0].is_ascii_alphanumeric()
        || !reference
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:/@-".contains(&c))
    {
        return Err("Referência de imagem inválida".into());
    }
    Ok(())
}
/// Compose service keys from the specification: no flags, spaces or shell syntax.
pub fn compose_service(name: &str) -> Res<()> {
    if name.is_empty()
        || name.len() > 128
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err("Nome de serviço Compose inválido".into());
    }
    Ok(())
}
/// Compose files named by the Compose conventions (compose.yaml, docker-compose.override.yml…).
fn compose_name(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file).to_ascii_lowercase();
    let stem = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"));
    stem.and_then(|s| s.strip_prefix("docker-").or(Some(s)))
        .is_some_and(|s| s == "compose" || s.starts_with("compose.") || s.starts_with("compose-"))
}
fn lines(text: &str) -> Res<Vec<Value>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(err))
        .collect()
}
impl Docker {
    /// Revoke a plugin generation before releasing UI capabilities. A new authorized activation must resume it.
    pub fn shutdown(&self) {
        if let Ok(mut operations) = self.operations.lock() {
            self.paused.store(true, Ordering::Release);
            for (_, signal) in operations.drain() {
                signal.notify_one();
            }
        }
    }
    pub fn resume(&self) {
        if let Ok(_operations) = self.operations.lock() {
            self.paused.store(false, Ordering::Release);
        }
    }
    pub fn executable(&self) -> &str {
        &self.program
    }
    async fn run(&self, root: &Path, args: &[String], seconds: u64) -> Res<(String, String)> {
        self.run_report(root, args, seconds, None).await
    }
    async fn run_report(
        &self,
        root: &Path,
        args: &[String],
        seconds: u64,
        progress: Option<(AppHandle, String)>,
    ) -> Res<(String, String)> {
        let operation_id = progress
            .as_ref()
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let signal = Arc::new(tokio::sync::Notify::new());
        {
            let mut ops = self.operations.lock().map_err(err)?;
            if self.paused.load(Ordering::Acquire) {
                return Err("Plugin Docker desativado".into());
            }
            if ops.contains_key(&operation_id) {
                return Err("Operação Docker já existe".into());
            }
            ops.insert(operation_id.clone(), signal.clone());
        }
        let _guard = Operation {
            owner: self,
            id: operation_id,
        };
        let mut child=Command::new(&self.program).args(args).current_dir(root).env("DOCKER_CLI_HINTS","false").env("COMPOSE_ANSI","never").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn().map_err(|e|format!("Docker CLI indisponível: {e}. Instale Docker Desktop ou adicione docker ao PATH."))?;
        let stdout = child.stdout.take().ok_or("stdout Docker ausente")?;
        let stderr = child.stderr.take().ok_or("stderr Docker ausente")?;
        let work = tokio::time::timeout(Duration::from_secs(seconds), async {
            let (out, error) =
                tokio::try_join!(capture(stdout, progress.clone()), capture(stderr, progress))?;
            let status = child.wait().await.map_err(err)?;
            if !status.success() {
                return Err(format!(
                    "Docker: {}",
                    if error.is_empty() { out } else { error }
                        .chars()
                        .take(3000)
                        .collect::<String>()
                ));
            }
            Ok((out, error))
        });
        tokio::select! {
            result=work=>result.map_err(|_|format!("Docker não respondeu em {seconds}s. Verifique se o engine está iniciado e se o contexto é acessível; tente conectar novamente."))?,
            _=signal.notified()=>Err("Operação Docker interrompida. Atualize os recursos para conferir o estado do engine; cancelar o CLI não reverte alterações já realizadas.".into())
        }
    }
    pub fn cancel(&self, id: &str) -> Res<Value> {
        uuid::Uuid::parse_str(id).map_err(err)?;
        let signal = self
            .operations
            .lock()
            .map_err(err)?
            .get(id)
            .cloned()
            .ok_or("Operação já finalizada ou ainda conectando. Tente novamente em instantes.")?;
        signal.notify_one();
        Ok(Value::Null)
    }
    pub async fn contexts(&self, root: &Path) -> Res<Vec<Value>> {
        let (out, _) = self
            .run(
                root,
                &[
                    "context".into(),
                    "ls".into(),
                    "--format".into(),
                    "{{json .}}".into(),
                ],
                8,
            )
            .await?;
        Ok(lines(&out)?
            .into_iter()
            .map(
                |v| json!({"name":v["Name"],"endpoint":v["DockerEndpoint"],"current":v["Current"]}),
            )
            .collect())
    }
    pub async fn validate_context(&self, root: &Path, context: &str) -> Res<()> {
        if context.len() > 255
            || !self
                .contexts(root)
                .await?
                .iter()
                .any(|v| v["name"] == context)
        {
            return Err("Contexto Docker não encontrado. Atualize a lista de conexões.".into());
        }
        Ok(())
    }
    async fn command(
        &self,
        root: &Path,
        context: &str,
        args: &[&str],
        seconds: u64,
    ) -> Res<(String, String)> {
        let mut argv = vec!["--context".into(), context.into()];
        argv.extend(args.iter().map(|s| s.to_string()));
        self.run(root, &argv, seconds).await
    }
    async fn command_report(
        &self,
        root: &Path,
        context: &str,
        args: &[&str],
        seconds: u64,
        progress: Option<(AppHandle, String)>,
    ) -> Res<(String, String)> {
        let mut argv = vec!["--context".into(), context.into()];
        argv.extend(args.iter().map(|s| s.to_string()));
        self.run_report(root, &argv, seconds, progress).await
    }
    pub fn compose_files(root: &Path) -> Vec<String> {
        [
            "compose.yaml",
            "compose.yml",
            "docker-compose.yaml",
            "docker-compose.yml",
        ]
        .iter()
        .filter(|p| confined(root, p).is_ok_and(|p| p.is_file()))
        .map(|s| s.to_string())
        .collect()
    }
    /// Root files from the selector, or any conventionally named Compose file inside the project.
    pub fn compose_file(root: &Path, file: &str) -> Res<()> {
        if Self::compose_files(root).iter().any(|f| f == file) {
            confined(root, file)?;
            return Ok(());
        }
        if file.starts_with('-')
            || Path::new(file).is_absolute()
            || file.contains('\\')
            || !compose_name(file)
            || !confined(root, file)?.is_file()
        {
            return Err("Selecione um arquivo Compose do projeto".into());
        }
        Ok(())
    }
    pub async fn dispatch(
        &self,
        root: &Path,
        app: &AppHandle,
        method: &str,
        args: &[Value],
    ) -> Res<Value> {
        if method == "dockerContexts" {
            return Ok(json!(self.contexts(root).await?));
        }
        if method == "dockerComposeFiles" {
            return Ok(json!(Self::compose_files(root)));
        }
        let get = |i| {
            args.get(i)
                .and_then(Value::as_str)
                .ok_or_else(|| "Parâmetro Docker inválido".to_string())
        };
        let context = get(0)?;
        self.validate_context(root, context).await?;
        match method {
            "dockerSnapshot" => {
                let (version, containers, images, volumes, networks) = tokio::try_join!(
                    self.command(
                        root,
                        context,
                        &["version", "--format", "{{json .Server}}"],
                        8
                    ),
                    self.command(
                        root,
                        context,
                        &[
                            "container",
                            "ls",
                            "--all",
                            "--no-trunc",
                            "--format",
                            "{{json .}}"
                        ],
                        8
                    ),
                    self.command(
                        root,
                        context,
                        &["image", "ls", "--no-trunc", "--format", "{{json .}}"],
                        8
                    ),
                    self.command(
                        root,
                        context,
                        &["volume", "ls", "--format", "{{json .}}"],
                        8
                    ),
                    self.command(
                        root,
                        context,
                        &["network", "ls", "--format", "{{json .}}"],
                        8
                    )
                )?;
                let version: Value = serde_json::from_str(&version.0).map_err(err)?;
                let containers=lines(&containers.0)?.into_iter().map(|v|json!({"id":v["ID"],"name":v["Names"],"image":v["Image"],"state":v["State"],"status":v["Status"],"ports":v["Ports"]})).collect::<Vec<_>>();
                let images=lines(&images.0)?.into_iter().map(|v|json!({"id":v["ID"],"repository":v["Repository"],"tag":v["Tag"],"size":v["Size"]})).collect::<Vec<_>>();
                let volumes = lines(&volumes.0)?
                    .into_iter()
                    .map(|v| json!({"name":v["Name"],"driver":v["Driver"]}))
                    .collect::<Vec<_>>();
                let networks = lines(&networks.0)?
                    .into_iter()
                    .map(|v| json!({"name":v["Name"],"driver":v["Driver"],"id":v["ID"]}))
                    .collect::<Vec<_>>();
                Ok(
                    json!({"version":version["Version"],"os":version["Os"],"containers":containers,"images":images,"volumes":volumes,"networks":networks}),
                )
            }
            "dockerLogs" => {
                let id = get(1)?;
                container_id(id)?;
                let (out, error) = self
                    .command(
                        root,
                        context,
                        &["logs", "--tail", "200", "--timestamps", id],
                        8,
                    )
                    .await?;
                Ok(json!(format!("{out}{error}")))
            }
            "dockerContainerAction" => {
                let id = get(1)?;
                container_id(id)?;
                let action = get(2)?;
                if !["start", "stop", "restart", "rm"].contains(&action) {
                    return Err("Ação Docker inválida".into());
                }
                let _lock = self.mutation.lock().await;
                let output = if action == "stop" || action == "restart" {
                    self.command(
                        root,
                        context,
                        &["container", action, "--time", "10", id],
                        30,
                    )
                    .await?
                } else {
                    self.command(root, context, &["container", action, id], 30)
                        .await?
                };
                Ok(json!(format!("{}{}", output.0, output.1)))
            }
            "dockerPull" => {
                let reference = get(1)?;
                image_reference(reference)?;
                let _lock = self.mutation.lock().await;
                let (out, error) = self
                    .command_report(
                        root,
                        context,
                        &["image", "pull", reference],
                        180,
                        progress(app, args.get(2))?,
                    )
                    .await?;
                Ok(json!(format!("{out}{error}")))
            }
            "dockerCompose" => {
                let file = get(1)?;
                Self::compose_file(root, file)?;
                let action = get(2)?;
                // Optional single service, used by the editor gutter (up/stop only).
                let service = args
                    .get(4)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                if let Some(service) = service {
                    compose_service(service)?;
                    if action != "up" && action != "stop" {
                        return Err("Ação Compose inválida".into());
                    }
                }
                let mut argv = vec!["compose", "--ansi", "never", "-f", file];
                match action {
                    "services" => argv.extend(["config", "--services"]),
                    "up" => argv.extend(["up", "-d"]),
                    "stop" => argv.push("stop"),
                    "build" => argv.push("build"),
                    _ => return Err("Ação Compose inválida".into()),
                }
                argv.extend(service);
                let _lock = self.mutation.lock().await;
                let (out, error) = self
                    .command_report(
                        root,
                        context,
                        &argv,
                        if action == "services" { 8 } else { 180 },
                        progress(app, args.get(3))?,
                    )
                    .await?;
                Ok(json!(format!("{out}{error}")))
            }
            _ => Err("Comando Docker desconhecido".into()),
        }
    }
}
fn progress(app: &AppHandle, value: Option<&Value>) -> Res<Option<(AppHandle, String)>> {
    match value.filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => {
            let id = v.as_str().ok_or("ID de operação inválido")?;
            uuid::Uuid::parse_str(id).map_err(err)?;
            Ok(Some((app.clone(), id.into())))
        }
    }
}
async fn capture(
    mut stream: impl tokio::io::AsyncRead + Unpin,
    progress: Option<(AppHandle, String)>,
) -> Res<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = stream.read(&mut chunk).await.map_err(err)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > 1048576 {
            return Err(
                "Saída Docker excedeu 1 MiB. Reduza o volume de logs e tente novamente.".into(),
            );
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some((app, id)) = &progress {
            let _ = app.emit(
                "docker-progress",
                json!({"id":id,"text":String::from_utf8_lossy(&chunk[..n])}),
            );
        }
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_cancels_unreported_commands_and_blocks_late_runs() {
        let docker = Arc::new(Docker {
            program: "/bin/sh".into(),
            ..Docker::default()
        });
        let owner = docker.clone();
        let task = tokio::spawn(async move {
            owner
                .run(
                    Path::new("/tmp"),
                    &["-c".into(), "exec sleep 30".into()],
                    40,
                )
                .await
        });
        for _ in 0..100 {
            if !docker.operations.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        docker.shutdown();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        assert!(docker
            .run(Path::new("/tmp"), &["-c".into(), "printf late".into()], 2)
            .await
            .is_err());
        docker.resume();
        assert_eq!(
            docker
                .run(
                    Path::new("/tmp"),
                    &["-c".into(), "printf resumed".into()],
                    2
                )
                .await
                .unwrap()
                .0,
            "resumed"
        );
    }
    #[test]
    fn compose_services_and_nested_files_are_validated() {
        for name in ["cache", "cache.read1", "app_1", "web-2"] {
            assert!(compose_service(name).is_ok(), "{name}");
        }
        for name in ["", "-d", "--build", "a b", "a;b", "../x", "$(id)"] {
            assert!(compose_service(name).is_err(), "{name}");
        }
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("docker")).unwrap();
        for f in [
            "docker/compose.yaml",
            "docker/docker-compose.override.yml",
            "docker/app.yml",
        ] {
            std::fs::write(root.join(f), "services: {}\n").unwrap();
        }
        assert!(Docker::compose_file(&root, "docker/compose.yaml").is_ok());
        assert!(Docker::compose_file(&root, "docker/docker-compose.override.yml").is_ok());
        for f in [
            "docker/app.yml",
            "missing/compose.yaml",
            "../compose.yaml",
            "-compose.yaml",
        ] {
            assert!(Docker::compose_file(&root, f).is_err(), "{f}");
        }
        let abs = root.join("docker/compose.yaml");
        assert!(Docker::compose_file(&root, abs.to_str().unwrap()).is_err());
    }
    #[test]
    fn arguments_are_not_shell_commands() {
        assert!(container_id("abcdef012345").is_ok());
        for id in ["--all", "abc; rm -rf /", "x", "../etc"] {
            assert!(container_id(id).is_err());
        }
        for image in [
            "php:8.3-cli",
            "registry.example/a/image:v1",
            "redis@sha256:abc",
        ] {
            assert!(image_reference(image).is_ok());
        }
        for image in ["--privileged", "php;echo x", "a b", ""] {
            assert!(image_reference(image).is_err());
        }
    }
    #[test]
    fn compose_paths_are_project_confined() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("compose.yaml"), "services: {}\n").unwrap();
        assert_eq!(
            Docker::compose_files(&d.path().canonicalize().unwrap()),
            vec!["compose.yaml"]
        );
    }
}
