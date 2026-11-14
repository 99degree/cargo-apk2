use crate::apk::UnalignedApk;
use crate::error::NdkError;
use crate::target::Target;
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

impl<'a> UnalignedApk<'a> {
    pub fn add_lib_recursively(
        &mut self,
        lib: &Path,
        target: Target,
        search_paths: &[&Path],
    ) -> Result<(), NdkError> {
        let ndk = &self.config().ndk;
        let default_min_sdk = crate::manifest::Sdk::default().min_sdk_version.unwrap();
        let min_sdk_version = self
            .config()
            .manifest
            .sdk
            .min_sdk_version
            .unwrap_or(default_min_sdk);
        let readelf_path = ndk.toolchain_bin("readelf", target)?;

        let android_search_paths = [
            &*ndk.sysroot_lib_dir(target)?,
            &*ndk.sysroot_platform_lib_dir(target, min_sdk_version)?,
        ];

        self.seed_provided(target, min_sdk_version)?;

        let mut stack: Vec<PathBuf> = vec![lib.to_path_buf()];
        while let Some(artifact) = stack.pop() {
            self.add_lib(&artifact, target)?;
            for need in Self::list_needed_libs_cached(&mut self.readelf_cache, &readelf_path, &artifact)? {
                // c++_shared 在 NDK 中可用，但在设备上不可用。如果使用，必须与 apk 捆绑在一起：
                // https://developer.android.com/ndk/guides/cpp-support#libc
                let search_paths = if need == "libc++_shared.so" {
                    &android_search_paths
                } else if !self.provided.contains(&need) {
                    search_paths
                } else {
                    continue;
                };

                if let Some(path) = Self::find_library_path(search_paths, &need)? {
                    let name = path.file_name().unwrap().to_str().unwrap().to_string();
                    if self.provided.insert(name.clone()) {
                        stack.push(path);
                    }
                } else {
                    eprintln!("Shared library \"{}\" not found.", need);
                }
            }
        }

        Ok(())
    }

    /// List linked shared libraries, reusing a cached readelf result when possible.
    fn list_needed_libs_cached(
        cache: &mut HashMap<PathBuf, HashSet<String>>,
        readelf_path: &Path,
        library_path: &Path,
    ) -> Result<HashSet<String>, NdkError> {
        if let Some(cached) = cache.get(library_path) {
            return Ok(cached.clone());
        }

        let mut readelf = Command::new(readelf_path);
        readelf.stdin(Stdio::null());
        let output = readelf.arg("-d").arg(library_path).output()?;
        if !output.status.success() {
            return Err(NdkError::CmdFailed(Box::new(readelf)));
        }
        let mut needed = HashSet::new();
        for line in output.stdout.lines() {
            let line = line?;
            if line.contains("(NEEDED)") {
                let lib = line
                    .split("Shared library: [")
                    .last()
                    .and_then(|line| line.split(']').next());
                if let Some(lib) = lib {
                    needed.insert(lib.to_string());
                }
            }
        }
        cache.insert(library_path.to_path_buf(), needed.clone());
        Ok(needed)
    }

    /// Initialize the `provided` set with libraries already available from the
    /// NDK sysroot so they are not re-analyzed or re-bundled.
    fn seed_provided(&mut self, target: Target, min_sdk_version: u32) -> Result<(), NdkError> {
        let ndk = &self.config().ndk;
        let android_search_paths = [
            &*ndk.sysroot_lib_dir(target)?,
            &*ndk.sysroot_platform_lib_dir(target, min_sdk_version)?,
        ];
        for path in &android_search_paths {
            for lib in Self::list_libs(path)? {
                if lib != "libc++_shared.so" {
                    self.provided.insert(lib);
                }
            }
        }
        Ok(())
    }

    /// List shared libraries
    fn list_libs(path: &Path) -> Result<HashSet<String>, NdkError> {
        let mut libs = HashSet::new();
        let entries = std::fs::read_dir(path)?;
        for entry in entries {
            let entry = entry?;
            if !entry.path().is_dir()
                && let Some(file_name) = entry.file_name().to_str()
                && file_name.ends_with(".so")
            {
                libs.insert(file_name.to_string());
            }
        }
        Ok(libs)
    }

    /// Resolves native library using search paths
    fn find_library_path<S: AsRef<Path>>(
        paths: &[&Path],
        library: S,
    ) -> Result<Option<PathBuf>, NdkError> {
        for path in paths {
            let lib_path = path.join(&library);
            if lib_path.exists() {
                return Ok(Some(dunce::canonicalize(lib_path)?));
            }
        }
        Ok(None)
    }
}
