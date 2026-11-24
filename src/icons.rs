//! Embedded SVG icons. gpui draws an SVG as a single-color mask tinted with the
//! element's `text_color`, so every icon is drawn in plain black.

use std::borrow::Cow;

use gpui::{AssetSource, Hsla, Pixels, Result, SharedString, Svg, prelude::*, svg};

#[derive(Clone, Copy)]
pub enum Icon {
    Folder,
    FolderOpen,
    File,
    ChevronRight,
    ChevronDown,
    Trash,
    Refresh,
    Drive,
    External,
    Shield,
    Close,
}

impl Icon {
    fn path(self) -> &'static str {
        match self {
            Icon::Folder => "icons/folder.svg",
            Icon::FolderOpen => "icons/folder-open.svg",
            Icon::File => "icons/file.svg",
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::ChevronDown => "icons/chevron-down.svg",
            Icon::Trash => "icons/trash.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::Drive => "icons/drive.svg",
            Icon::External => "icons/external.svg",
            Icon::Shield => "icons/shield.svg",
            Icon::Close => "icons/close.svg",
        }
    }
}

/// An icon element of the given size and color.
pub fn icon(icon: Icon, size: Pixels, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(icon.path())
        .size(size)
        .flex_shrink_0()
        .text_color(color.into())
}

macro_rules! embed {
    ($($name:literal),* $(,)?) => {
        fn lookup(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("icons/", $name, ".svg") =>
                    Some(include_bytes!(concat!("../assets/icons/", $name, ".svg"))),)*
                _ => None,
            }
        }
    };
}

embed!(
    "folder",
    "folder-open",
    "file",
    "chevron-right",
    "chevron-down",
    "trash",
    "refresh",
    "drive",
    "external",
    "shield",
    "close",
);

/// Serves the icons compiled into the binary, so the exe has no loose files.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(lookup(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(["folder", "folder-open", "file", "chevron-right", "chevron-down", "trash",
            "refresh", "drive", "external", "shield", "close"]
            .iter()
            .map(|n| format!("icons/{n}.svg"))
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}
