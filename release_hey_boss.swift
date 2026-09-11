import Foundation
import CryptoKit

func releaseHeyBoss(_ version: String, _ repository: String) {
    precondition(!version.isEmpty && version.allSatisfy { $0.isNumber || $0 == "." })
    precondition(repository.split(separator: "/").count == 2)
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    let manifest = try! String(contentsOf: root.appendingPathComponent("Cargo.toml"), encoding: .utf8)
    precondition(manifest.components(separatedBy: .newlines).contains("version = \"\(version)\""))
    try! FileManager.default.createDirectory(at: root.appendingPathComponent("out"), withIntermediateDirectories: true)
    let archive = "hey-boss-\(version)-source.tar.gz"
    let path = root.appendingPathComponent("out/\(archive)")
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/tar")
    process.arguments = ["--uid", "0", "--gid", "0", "--uname", "root", "--gname", "root", "-czf", path.path, "-C", root.path, "Cargo.toml", "Cargo.lock", "src", "hey_boss_daemon.swift", "setup_hey_boss.swift", "install_hey_boss.swift", "test_hey_boss.swift", "release_hey_boss.swift", "LICENSE", "README.md", "CONTRIBUTING.md", "SECURITY.md", "skills"]
    process.environment = ProcessInfo.processInfo.environment.merging(["COPYFILE_DISABLE": "1"]) { _, replacement in replacement }
    try! process.run()
    process.waitUntilExit()
    precondition(process.terminationStatus == 0)
    let checksum = SHA256.hash(data: try! Data(contentsOf: path)).map { String(format: "%02x", $0) }.joined()
    let formula = """
    class HeyBoss < Formula
      desc "Native project updates, notifications, and questions for coding agents"
      homepage "https://github.com/\(repository)"
      url "https://github.com/\(repository)/releases/download/v\(version)/\(archive)"
      sha256 "\(checksum)"
      license "MIT"

      depends_on "rust" => :build
      depends_on macos: :tahoe

      def install
        system "cargo", "install", *std_cargo_args(root: libexec)
        system "xcrun", "swiftc", "-O", "-whole-module-optimization", "-parse-as-library",
               "hey_boss_daemon.swift", "-o", "hey-boss-daemon"
        system "xcrun", "swiftc", "-O", "setup_hey_boss.swift", "-o", "hey-boss-setup"
        libexec.install "hey-boss-daemon", "hey-boss-setup"
        bin.install_symlink libexec/"bin/hey-boss"
        pkgshare.install "skills"
      end

      def post_install
        state = Pathname(Dir.home(ENV.fetch("USER")))/"Library/Application Support/hey-boss"
        agents = Pathname(Dir.home(ENV.fetch("USER")))/"Library/LaunchAgents"
        require "plist"

        (libexec/"bin/hey-boss.setup").atomic_write(
          [opt_libexec/"hey-boss-setup", state, libexec/"bin", agents,
           opt_libexec/"hey-boss-daemon"].join("\n"),
        )
        (libexec/"bin/hey-boss.setup").chmod 0600
        (prefix/"local.hey-boss.plist").atomic_write({
          "Label"                  => "local.hey-boss",
          "ProgramArguments"       => [(opt_libexec/"hey-boss-daemon").to_s],
          "EnvironmentVariables"   => { "HEY_BOSS_STATE_DIR" => state.to_s },
          "LimitLoadToSessionType" => "Aqua",
          "Sockets"                => { "Listener" => { "SockPathName" => (state/"daemon.sock").to_s,
                                         "SockPathMode" => 0600, "SockType" => "stream" } },
          "StandardOutPath"        => (state/"daemon.log").to_s,
          "StandardErrorPath"      => (state/"daemon.log").to_s,
          "ProcessType"            => "Interactive",
        }.to_plist)
      end

      service do
        name macos: "local.hey-boss"
      end

      def caveats
        <<~EOS
          The first notification or question registers the daemon automatically.
          History stays in ~/Library/Application Support/hey-boss.
          Before uninstalling, run: brew services stop hey-boss
        EOS
      end

      test do
        assert_equal "hey-boss #{version}\\n", shell_output("#{bin}/hey-boss --version")
        assert_match "--project", shell_output("#{bin}/hey-boss update --help")
      end
    end
    """
    try! Data((formula + "\n").utf8).write(to: root.appendingPathComponent("out/hey-boss.rb"), options: .atomic)
    try! Data("\(checksum)  \(archive)\n".utf8).write(to: root.appendingPathComponent("out/SHA256SUMS"), options: .atomic)
    print(path.path)
    print("SHA256: \(checksum)")
}

precondition(CommandLine.arguments.count == 3)
releaseHeyBoss(CommandLine.arguments[1], CommandLine.arguments[2])
