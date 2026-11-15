use dmz_core::resolver::{ResolveOptions, ResolverEngine};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_npm_package_lock_excludes_dev_and_foreign_os_by_default(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let root = tmp.path();

    fs::write(
        root.join("package.json"),
        r#"{
            "name": "web-app",
            "version": "1.0.0",
            "dependencies": { "express": "^4.18.0" },
            "devDependencies": { "vitest": "^1.0.0" }
        }"#,
    )?;

    fs::write(
        root.join("package-lock.json"),
        r#"{
            "name": "web-app",
            "lockfileVersion": 3,
            "packages": {
                "": {
                    "name": "web-app",
                    "dependencies": { "express": "^4.18.0" },
                    "devDependencies": { "vitest": "^1.0.0" }
                },
                "node_modules/express": {
                    "version": "4.18.2"
                },
                "node_modules/vitest": {
                    "version": "1.6.0",
                    "dev": true
                },
                "node_modules/fsevents": {
                    "version": "2.3.3",
                    "optional": true,
                    "os": ["darwin"]
                }
            }
        }"#,
    )?;

    let linux_prod = ResolverEngine::resolve_workspace_with_options(
        root,
        ResolveOptions {
            target_platform: Some("linux-x64".to_string()),
            include_dev: false,
        },
    )?;
    let linux_names: Vec<&str> = linux_prod
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(linux_names.contains(&"express"));
    assert!(!linux_names.contains(&"vitest"));
    assert!(!linux_names.contains(&"fsevents"));

    let darwin_dev = ResolverEngine::resolve_workspace_with_options(
        root,
        ResolveOptions {
            target_platform: Some("macos-arm64".to_string()),
            include_dev: true,
        },
    )?;
    let darwin_names: Vec<&str> = darwin_dev
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(darwin_names.contains(&"express"));
    assert!(darwin_names.contains(&"vitest"));
    assert!(darwin_names.contains(&"fsevents"));

    Ok(())
}

#[test]
fn test_pnpm_lock_prunes_dev_closure_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let root = tmp.path();

    fs::write(
        root.join("package.json"),
        r#"{
            "name": "pnpm-app",
            "dependencies": { "axios": "^1.6.0" },
            "devDependencies": { "eslint": "^8.0.0" }
        }"#,
    )?;

    fs::write(
        root.join("pnpm-lock.yaml"),
        r#"lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      axios:
        specifier: ^1.6.0
        version: 1.6.8
    devDependencies:
      eslint:
        specifier: ^8.0.0
        version: 8.57.0
packages:
  axios@1.6.8:
    resolution: {integrity: sha512-axios}
  eslint@8.57.0:
    resolution: {integrity: sha512-eslint}
"#,
    )?;

    let prod = ResolverEngine::resolve_workspace_with_options(
        root,
        ResolveOptions {
            target_platform: None,
            include_dev: false,
        },
    )?;
    let prod_names: Vec<&str> = prod.packages.values().map(|p| p.name.as_str()).collect();
    assert!(prod_names.contains(&"axios"));
    assert!(!prod_names.contains(&"eslint"));

    let with_dev = ResolverEngine::resolve_workspace_with_options(
        root,
        ResolveOptions {
            target_platform: None,
            include_dev: true,
        },
    )?;
    let dev_names: Vec<&str> = with_dev
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(dev_names.contains(&"axios"));
    assert!(dev_names.contains(&"eslint"));

    Ok(())
}

#[test]
fn test_python_markers_optional_and_lockfile_dev_exclusion(
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. requirements.txt + pyproject.toml markers and optional-dependencies
    let tmp_req = tempdir()?;
    let req_root = tmp_req.path();
    fs::write(
        req_root.join("requirements.txt"),
        r#"
requests==2.31.0
pywin32==306; sys_platform == "win32"
uvloop==0.19.0; platform_system == "Linux"
sphinx==7.2.6; extra == "docs"
"#,
    )?;
    fs::write(
        req_root.join("pyproject.toml"),
        r#"
[project]
name = "py-app"
version = "0.1.0"
dependencies = ["httpx>=0.27.0"]

[project.optional-dependencies]
dev = ["pytest>=8.0.0"]
"#,
    )?;

    let linux_lock = ResolverEngine::resolve_workspace_with_options(
        req_root,
        ResolveOptions {
            target_platform: Some("linux-x64".to_string()),
            include_dev: false,
        },
    )?;
    let linux_names: Vec<&str> = linux_lock
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(linux_names.contains(&"requests"));
    assert!(linux_names.contains(&"httpx"));
    assert!(linux_names.contains(&"uvloop"));
    assert!(!linux_names.contains(&"pywin32"));
    assert!(!linux_names.contains(&"sphinx"));
    assert!(!linux_names.contains(&"pytest"));

    // 2. poetry.lock excludes non-main groups unless include_dev is true
    let tmp_poetry = tempdir()?;
    fs::write(
        tmp_poetry.path().join("poetry.lock"),
        r#"
[[package]]
name = "fastapi"
version = "0.111.0"
category = "main"

[[package]]
name = "pytest"
version = "8.2.0"
groups = ["dev"]
"#,
    )?;
    let poetry_prod = ResolverEngine::resolve_workspace_with_options(
        tmp_poetry.path(),
        ResolveOptions {
            include_dev: false,
            ..Default::default()
        },
    )?;
    let poetry_prod_names: Vec<&str> = poetry_prod
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(poetry_prod_names.contains(&"fastapi"));
    assert!(!poetry_prod_names.contains(&"pytest"));

    let poetry_dev = ResolverEngine::resolve_workspace_with_options(
        tmp_poetry.path(),
        ResolveOptions {
            include_dev: true,
            ..Default::default()
        },
    )?;
    let poetry_dev_names: Vec<&str> = poetry_dev
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(poetry_dev_names.contains(&"fastapi"));
    assert!(poetry_dev_names.contains(&"pytest"));

    // 3. uv.lock walks runtime closure cleanly and drops dev-dependencies by default
    let tmp_uv = tempdir()?;
    fs::write(
        tmp_uv.path().join("uv.lock"),
        r#"
version = 1

[[package]]
name = "my-service"
version = "0.1.0"
source = { virtual = "." }
dependencies = [{ name = "uvicorn" }]

[package.dev-dependencies]
dev = [{ name = "ruff" }]

[[package]]
name = "uvicorn"
version = "0.30.0"

[[package]]
name = "ruff"
version = "0.4.8"
"#,
    )?;
    let uv_prod = ResolverEngine::resolve_workspace_with_options(
        tmp_uv.path(),
        ResolveOptions {
            include_dev: false,
            ..Default::default()
        },
    )?;
    let uv_prod_names: Vec<&str> = uv_prod.packages.values().map(|p| p.name.as_str()).collect();
    assert!(uv_prod_names.contains(&"uvicorn"));
    assert!(!uv_prod_names.contains(&"ruff"));

    let uv_dev = ResolverEngine::resolve_workspace_with_options(
        tmp_uv.path(),
        ResolveOptions {
            include_dev: true,
            ..Default::default()
        },
    )?;
    let uv_dev_names: Vec<&str> = uv_dev.packages.values().map(|p| p.name.as_str()).collect();
    assert!(uv_dev_names.contains(&"uvicorn"));
    assert!(uv_dev_names.contains(&"ruff"));

    Ok(())
}

#[test]
fn test_go_mod_excludes_indirect_when_only_direct_required_in_offline_fallback(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    fs::write(
        tmp.path().join("go.mod"),
        r#"
module example.com/myservice

go 1.22.0
toolchain go1.22.1

require (
	github.com/gin-gonic/gin v1.9.1
	golang.org/x/sys v0.18.0 // indirect
)
"#,
    )?;

    let lock =
        ResolverEngine::resolve_workspace_with_options(tmp.path(), ResolveOptions::default())?;
    let names: Vec<&str> = lock.packages.values().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"github.com/gin-gonic/gin"));
    assert!(!names.iter().any(|n| n.contains("toolchain")));

    Ok(())
}

#[test]
fn test_composer_bundler_and_maven_exclude_dev_by_default() -> Result<(), Box<dyn std::error::Error>>
{
    // Composer
    let tmp_composer = tempdir()?;
    fs::write(
        tmp_composer.path().join("composer.json"),
        r#"{
            "name": "acme/php-app",
            "require": { "php": "^8.2", "monolog/monolog": "^3.5" },
            "require-dev": { "phpunit/phpunit": "^11.0" }
        }"#,
    )?;
    let composer_prod = ResolverEngine::resolve_workspace_with_options(
        tmp_composer.path(),
        ResolveOptions {
            include_dev: false,
            ..Default::default()
        },
    )?;
    let composer_prod_names: Vec<&str> = composer_prod
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(composer_prod_names.contains(&"monolog/monolog"));
    assert!(!composer_prod_names.contains(&"phpunit/phpunit"));

    let composer_dev = ResolverEngine::resolve_workspace_with_options(
        tmp_composer.path(),
        ResolveOptions {
            include_dev: true,
            ..Default::default()
        },
    )?;
    assert!(composer_dev
        .packages
        .values()
        .any(|p| p.name == "phpunit/phpunit"));

    // Bundler
    let tmp_bundler = tempdir()?;
    fs::write(
        tmp_bundler.path().join("Gemfile"),
        r#"
source "https://rubygems.org"
gem "sinatra", "~> 4.0"

group :development, :test do
  gem "rspec", "~> 3.13"
end
"#,
    )?;
    let bundler_prod = ResolverEngine::resolve_workspace_with_options(
        tmp_bundler.path(),
        ResolveOptions {
            include_dev: false,
            ..Default::default()
        },
    )?;
    let bundler_prod_names: Vec<&str> = bundler_prod
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(bundler_prod_names.contains(&"sinatra"));
    assert!(!bundler_prod_names.contains(&"rspec"));

    let bundler_dev = ResolverEngine::resolve_workspace_with_options(
        tmp_bundler.path(),
        ResolveOptions {
            include_dev: true,
            ..Default::default()
        },
    )?;
    assert!(bundler_dev.packages.values().any(|p| p.name == "rspec"));

    // Maven
    let tmp_maven = tempdir()?;
    fs::write(
        tmp_maven.path().join("pom.xml"),
        r#"<project>
  <modelVersion>4.0.0</modelVersion>
  <groupId>com.example</groupId>
  <artifactId>demo</artifactId>
  <version>1.0.0</version>
  <dependencies>
    <dependency>
      <groupId>org.slf4j</groupId>
      <artifactId>slf4j-api</artifactId>
      <version>2.0.12</version>
    </dependency>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter</artifactId>
      <version>5.10.2</version>
      <scope>test</scope>
    </dependency>
  </dependencies>
</project>"#,
    )?;
    let maven_prod = ResolverEngine::resolve_workspace_with_options(
        tmp_maven.path(),
        ResolveOptions {
            include_dev: false,
            ..Default::default()
        },
    )?;
    let maven_prod_names: Vec<&str> = maven_prod
        .packages
        .values()
        .map(|p| p.name.as_str())
        .collect();
    assert!(maven_prod_names.contains(&"org.slf4j:slf4j-api"));
    assert!(!maven_prod_names.contains(&"org.junit.jupiter:junit-jupiter"));

    let maven_dev = ResolverEngine::resolve_workspace_with_options(
        tmp_maven.path(),
        ResolveOptions {
            include_dev: true,
            ..Default::default()
        },
    )?;
    assert!(maven_dev
        .packages
        .values()
        .any(|p| p.name == "org.junit.jupiter:junit-jupiter"));

    Ok(())
}
