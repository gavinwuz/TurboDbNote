use crate::preferences::Preferences;
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, StyledExt,
    button::{Button, ButtonVariants},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use turbodbn_services::mysql_drivers;

pub struct DriverManager {
    pub components: Entity<crate::driver_components::DriverComponents>,
    root: PathBuf,
    details: bool,
    path: Option<PathBuf>,
    helper_available: bool,
    status: String,
    cancel: Option<Arc<AtomicBool>>,
    task: Option<Task<()>>,
}
impl DriverManager {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let root = Preferences::path()
            .unwrap_or_else(|_| PathBuf::from("settings.json"))
            .with_file_name("drivers");
        let local = mysql_drivers::installed(&root);
        let path = mysql_drivers::bundled()
            .filter(|p| mysql_drivers::validate_pinned(p).is_ok())
            .or_else(|| {
                mysql_drivers::validate_pinned(&local)
                    .is_ok()
                    .then_some(local)
            });
        Self {
            components: cx.new(crate::driver_components::DriverComponents::new),
            helper_available: std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("turbodbnote-mysql-legacy.exe")))
                .is_some_and(|p| p.is_file()),
            root,
            details: false,
            path,
            status: "MySQL 4.0 / 4.1 / 早期 5.x · 客户端 5.1.73 x64".into(),
            cancel: None,
            task: None,
        }
    }
    pub fn available(&self) -> bool {
        self.path.is_some() && self.helper_available
    }
    pub fn status_key(&self) -> &'static str {
        if !self.helper_available {
            "drivers.helper_missing"
        } else if self.path.is_none() {
            "drivers.client_missing"
        } else {
            "drivers.compat_ready"
        }
    }
    pub fn path(&self) -> Option<PathBuf> {
        self.path.clone()
    }
    fn import(&mut self, cx: &mut Context<Self>) {
        if self.cancel.is_some() {
            return;
        }
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择官方 mysql-noinstall-5.1.73-winx64.zip".into()),
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(paths))) = selection.await
                && let Some(path) = paths.first()
            {
                let _ = view.update(cx, |this, cx| this.start(Some(path.clone()), cx));
            }
        })
        .detach();
    }
    fn start(&mut self, archive: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.cancel.is_some() {
            return;
        }
        if !self.helper_available {
            crate::feedback::error(crate::locale::t(cx, "drivers.helper_missing"), cx);
            return;
        }
        if archive.is_none()
            && self
                .path
                .as_ref()
                .is_some_and(|path| mysql_drivers::validate_pinned(path).is_ok())
        {
            crate::feedback::success(crate::locale::t(cx, "drivers.compat_ready"), cx);
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.status = "准备驱动…".into();
        let root = self.root.clone();
        let (send, recv) = async_channel::unbounded::<Result<String, Result<PathBuf, String>>>();
        std::thread::spawn(move || {
            let result = mysql_drivers::install(&root, archive.as_deref(), &cancel, |s| {
                let _ = send.try_send(Ok(s));
            });
            let _ = send.try_send(Err(result));
        });
        self.task = Some(cx.spawn(async move |view, cx| {
            while let Ok(event) = recv.recv().await {
                if view
                    .update(cx, |this, cx| {
                        match event {
                            Ok(status) => this.status = status,
                            Err(result) => {
                                this.cancel = None;
                                match result {
                                    Ok(path) => {
                                        this.path = Some(path);
                                        this.status =
                                            "驱动已就绪，连接时自动使用；无需填写 DLL 路径".into();
                                    }
                                    Err(error) => {
                                        this.status =
                                            format!("未安装：{error}。可重试或导入离线 ZIP。")
                                    }
                                }
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }
}
impl Drop for DriverManager {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl Render for DriverManager {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.cancel.is_some();
        div().v_flex().gap_3().p_3().rounded_md().border_1().border_color(cx.theme().border)
        .child(div().text_lg().child("数据库驱动"))
        .child("MySQL 4.x / 早期 5.x 兼容驱动")
        .child(div().text_sm().text_color(cx.theme().muted_foreground).child(self.status.clone()))
        .child(div().h_flex().gap_2()
            .child(gpui_component::Icon::new(if self.available(){IconName::CircleCheck}else{IconName::Info}))
            .child(crate::locale::t(cx,self.status_key())))
        .child(Button::new("driver-details").ghost().icon(IconName::Info).label(if self.details {"收起详情"}else{"驱动详情"})
            .on_click(cx.listener(|this,_,_,cx|{this.details = !this.details;cx.notify();})))
        .when(self.details,|el|el.child(div().v_flex().gap_2().text_sm()
            .child(self.path.as_ref().map(|p|format!("存储位置：{}",p.display())).unwrap_or("尚未安装".into()))
            .when_some(self.path.clone(),|el,path|{let copy=path.clone();el.child(div().h_flex().gap_2()
                .child(Button::new("copy-driver-path").icon(IconName::Copy).label("复制路径").on_click(move |_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(copy.display().to_string()))))
                .child(Button::new("reveal-driver-path").icon(IconName::FolderOpen).label("打开位置").on_click(move |_,_,cx|cx.reveal_path(&path))))})))
        .child(div().h_flex().gap_2().flex_wrap()
            .child(Button::new("download-mysql-driver").icon(IconName::ArrowDown).primary().label(if self.path.is_some() { "检查 / 修复" } else { "下载驱动" }).disabled(busy).on_click(cx.listener(|this,_,_,cx|this.start(None,cx))))
            .child(Button::new("import-mysql-driver").icon(IconName::FolderOpen).label("导入官方 ZIP").disabled(busy).on_click(cx.listener(|this,_,_,cx|this.import(cx))))
            .when(busy,|el|el.child(Button::new("cancel-mysql-download").icon(IconName::Close).label("取消下载").disabled(!busy).on_click(cx.listener(|this,_,_,cx|{if let Some(cancel)=&this.cancel {cancel.store(true,Ordering::Relaxed);this.status="正在取消…".into();cx.notify();}})))))
        .child(div().text_sm().text_color(cx.theme().muted_foreground).child("自动校验 SHA-256；仅安装客户端，不安装数据库服务。自定义 DLL 可在连接的高级页指定。"))
    }
}
