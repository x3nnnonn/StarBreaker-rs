pub fn is_socpak_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.contains("shadercache_") {
        return false;
    }
    let name = lower.rsplit(['\\', '/']).next().unwrap_or(&lower);
    name.ends_with(".socpak") || name.ends_with(".pak")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_socpak_path_matches_csharp() {
        assert!(is_socpak_path(r"Data\ObjectContainers\ships\foo.socpak"));
        assert!(is_socpak_path(r"Data\Levels\bar.pak"));
        assert!(!is_socpak_path(r"Data\shadercache_foo.socpak"));
        assert!(!is_socpak_path(r"Data\Textures\baz.dds"));
    }
}
