import AppKit
import Darwin
import Foundation
import UniformTypeIdentifiers
import WebKit

private let kPythonStandaloneURL =
    "https://github.com/astral-sh/python-build-standalone/releases/download/20261110/cpython-3.12.16%2B20261110-aarch64-apple-darwin-install_only_stripped.tar.gz"
private let kOllamaZipURL = "https://ollama.com/download/Ollama-darwin.zip"
private let kOllamaBaseURL = "http://127.0.0.1:11434"
private let kServerBaseURL = "http://127.0.0.1:8765"

private let kLongTimeoutSession: URLSession = {
    let cfg = URLSessionConfiguration.default
    cfg.timeoutIntervalForRequest = 300
    cfg.timeoutIntervalForResource = 300
    return URLSession(configuration: cfg)
}()

func hostArchitecture() -> String {
    var sysinfo = utsname()
    uname(&sysinfo)
    return withUnsafePointer(to: &sysinfo.machine) {
        $0.withMemoryRebound(to: CChar.self, capacity: Int(_SYS_NAMELEN)) {
            String(cString: $0)
        }
    }
}

func sysctlString(_ name: String) -> String {
    var size: Int = 0
    sysctlbyname(name, nil, &size, nil, 0)
    guard size > 0 else { return "" }
    var buf = [CChar](repeating: 0, count: size)
    sysctlbyname(name, &buf, &size, nil, 0)
    return String(cString: buf)
}

func sysctlUInt64(_ name: String) -> UInt64 {
    var val: UInt64 = 0
    var size = MemoryLayout<UInt64>.size
    sysctlbyname(name, &val, &size, nil, 0)
    return val
}

func appSupportDataDir() -> URL {
    if let env = ProcessInfo.processInfo.environment["SMT_DATA_DIR"], !env.isEmpty {
        return URL(fileURLWithPath: (env as NSString).expandingTildeInPath, isDirectory: true)
    }
    let fm = FileManager.default
    let appSupport = fm.homeDirectoryForCurrentUser
        .appendingPathComponent("Library", isDirectory: true)
        .appendingPathComponent("Application Support", isDirectory: true)
    let yomiDir = appSupport.appendingPathComponent("Yomi", isDirectory: true)
    let legacyDir = appSupport.appendingPathComponent("MangaTranslator", isDirectory: true)
    if !fm.fileExists(atPath: yomiDir.path), fm.fileExists(atPath: legacyDir.path) {
        try? fm.moveItem(at: legacyDir, to: yomiDir)
    }
    return yomiDir
}

func resolveResourceURL(_ relativePath: String) -> URL? {
    let fm = FileManager.default
    if let res = Bundle.main.resourceURL {
        let candidate = res.appendingPathComponent(relativePath)
        if fm.fileExists(atPath: candidate.path) {
            return candidate
        }
    }
    let execDir = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath().deletingLastPathComponent()
    let devRoots = [
        execDir,
        execDir.deletingLastPathComponent(),
        URL(fileURLWithPath: fm.currentDirectoryPath, isDirectory: true),
    ]
    for root in devRoots {
        let candidate = root.appendingPathComponent(relativePath)
        if fm.fileExists(atPath: candidate.path) {
            return candidate
        }
    }
    return nil
}

struct ProcessResult {
    let exitCode: Int32
    let stdout: String
    let stderr: String
}

func runSyncProcess(_ executable: String, _ args: [String], env: [String: String]? = nil) -> ProcessResult {
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: executable)
    proc.arguments = args
    if let env = env {
        var merged = ProcessInfo.processInfo.environment
        for (k, v) in env { merged[k] = v }
        proc.environment = merged
    }
    let outPipe = Pipe()
    let errPipe = Pipe()
    proc.standardOutput = outPipe
    proc.standardError = errPipe
    do {
        try proc.run()
        proc.waitUntilExit()
        let out = String(data: outPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let err = String(data: errPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return ProcessResult(exitCode: proc.terminationStatus, stdout: out.trimmingCharacters(in: .whitespacesAndNewlines), stderr: err.trimmingCharacters(in: .whitespacesAndNewlines))
    } catch {
        return ProcessResult(exitCode: -1, stdout: "", stderr: error.localizedDescription)
    }
}

func httpJSONSync(urlString: String, method: String = "GET", body: [String: Any]? = nil, headers: [String: String] = [:], timeout: TimeInterval = 2.0) -> (Int, [String: Any]?, String) {
    guard let url = URL(string: urlString) else { return (0, nil, "Invalid URL") }
    var req = URLRequest(url: url, timeoutInterval: timeout)
    req.httpMethod = method
    for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
    if let body = body {
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: body)
    }
    final class Box: @unchecked Sendable {
        var status = 0
        var json: [String: Any]? = nil
        var error = ""
    }
    let box = Box()
    let sem = DispatchSemaphore(value: 0)
    let session = timeout > 30 ? kLongTimeoutSession : URLSession.shared
    session.dataTask(with: req) { data, resp, err in
        if let http = resp as? HTTPURLResponse { box.status = http.statusCode }
        if let err = err { box.error = err.localizedDescription }
        if let data = data, let parsed = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            box.json = parsed
        } else if let data = data, let txt = String(data: data, encoding: .utf8), box.error.isEmpty {
            box.error = String(txt.prefix(240))
        }
        sem.signal()
    }.resume()
    _ = sem.wait(timeout: .now() + timeout + 0.5)
    return (box.status, box.json, box.error)
}

func detectPython(dataDir: URL) -> [String: Any] {
    let fm = FileManager.default
    var candidates: [(String, String)] = []
    let bundledPy = dataDir
        .appendingPathComponent("runtime/python/bin/python3").path
    candidates.append((bundledPy, "app"))
    candidates.append(("/opt/homebrew/bin/python3", "homebrew"))

    let fwBase = "/Library/Frameworks/Python.framework/Versions"
    if let vers = try? fm.contentsOfDirectory(atPath: fwBase) {
        for v in vers.sorted(by: >) {
            candidates.append(("\(fwBase)/\(v)/bin/python3", "framework"))
        }
    }
    candidates.append(("/usr/local/bin/python3", "system"))
    candidates.append(("/usr/bin/python3", "apple"))

    var seen = Set<String>()
    for (path, source) in candidates {
        if seen.contains(path) || !fm.isExecutableFile(atPath: path) { continue }
        seen.insert(path)
        let res = runSyncProcess(path, ["-c", "import sys, json; print(f'{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}')"])
        if res.exitCode == 0 && !res.stdout.isEmpty {
            let parts = res.stdout.split(separator: ".").compactMap { Int($0) }
            if parts.count >= 2 && (parts[0] > 3 || (parts[0] == 3 && parts[1] >= 10)) {
                return [
                    "installed": true,
                    "path": path,
                    "version": res.stdout,
                    "source": source,
                ]
            }
        }
    }
    return ["installed": false, "path": "", "version": "", "source": ""]
}

func findOllamaBinary() -> (String, String?) {
    let fm = FileManager.default
    let home = fm.homeDirectoryForCurrentUser.path
    let binCandidates = [
        "/opt/homebrew/bin/ollama",
        "/usr/local/bin/ollama",
        "/Applications/Ollama.app/Contents/Resources/ollama",
        "\(home)/Applications/Ollama.app/Contents/Resources/ollama",
        "/Applications/Ollama.app/Contents/MacOS/ollama",
        "\(home)/Applications/Ollama.app/Contents/MacOS/ollama",
    ]
    let appCandidates = [
        "/Applications/Ollama.app",
        "\(home)/Applications/Ollama.app",
    ]
    var foundBin = ""
    for p in binCandidates {
        if fm.isExecutableFile(atPath: p) {
            foundBin = p
            break
        }
    }
    var foundApp: String? = nil
    for a in appCandidates {
        if fm.fileExists(atPath: a) {
            foundApp = a
            break
        }
    }
    return (foundBin, foundApp)
}

func detectOllama() -> [String: Any] {
    let (binPath, appPath) = findOllamaBinary()
    let (status, tagsJson, _) = httpJSONSync(urlString: "\(kOllamaBaseURL)/api/tags", timeout: 1.5)
    let running = (status == 200)
    var models: [String] = []
    if let rawModels = tagsJson?["models"] as? [[String: Any]] {
        for m in rawModels {
            if let name = m["name"] as? String, !name.isEmpty {
                models.append(name)
            }
        }
    }
    let installed = running || !binPath.isEmpty || appPath != nil
    return [
        "installed": installed,
        "path": binPath,
        "app_path": appPath ?? "",
        "running": running,
        "models": models,
    ]
}

func detectOCRReady() -> Bool {
    let fm = FileManager.default
    if let res = Bundle.main.resourceURL {
        let ocr = res.appendingPathComponent("ocr/vision_ocr").path
        let som = res.appendingPathComponent("ocr/annotate_som").path
        if fm.isExecutableFile(atPath: ocr) && fm.isExecutableFile(atPath: som) {
            return true
        }
    }
    if let v = resolveResourceURL("ocr/vision_ocr"), let a = resolveResourceURL("ocr/annotate_som") {
        return fm.isExecutableFile(atPath: v.path) && fm.isExecutableFile(atPath: a.path)
    }
    return false
}

func maskKey(_ key: String) -> String {
    let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty { return "" }
    if trimmed.count <= 8 { return String(repeating: "*", count: trimmed.count) }
    return "\(trimmed.prefix(4))...\(trimmed.suffix(4))"
}

private let kProviderDefaults: [String: (baseURL: String, defaultModel: String, envVars: [String])] = [
    "gemini": ("https://generativelanguage.googleapis.com/v1beta", "gemini-3.8-flash", ["GEMINI_API_KEY", "YOMI_API_KEY"]),
    "openai": ("https://api.openai.com/v1", "gpt-4.1-mini", ["OPENAI_API_KEY", "YOMI_API_KEY"]),
    "anthropic": ("https://api.anthropic.com/v1", "claude-sonnet-4-5", ["ANTHROPIC_API_KEY", "YOMI_API_KEY"]),
    "openrouter": ("https://openrouter.ai/api/v1", "google/gemini-2.5-flash", ["OPENROUTER_API_KEY", "YOMI_API_KEY"]),
    "mistral": ("https://api.mistral.ai/v1", "mistral-medium-latest", ["MISTRAL_API_KEY", "YOMI_API_KEY"]),
    "deepseek": ("https://api.deepseek.com/v1", "deepseek-chat", ["DEEPSEEK_API_KEY", "YOMI_API_KEY"]),
    "groq": ("https://api.groq.com/openai/v1", "meta-llama/llama-4-scout-17b-16e-instruct", ["GROQ_API_KEY", "YOMI_API_KEY"]),
    "xai": ("https://api.x.ai/v1", "grok-2-vision-latest", ["XAI_API_KEY", "YOMI_API_KEY"]),
    "custom": ("", "", ["YOMI_API_KEY", "OPENAI_API_KEY"]),
]

func effectiveCloudApiKey(cfg: [String: Any], provider overrideProvider: String? = nil) -> String {
    let activeProv = ((cfg["api_provider"] as? String) ?? "gemini").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    let provRaw = (overrideProvider ?? activeProv).trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    let prov = kProviderDefaults[provRaw] != nil ? provRaw : "gemini"

    if prov == activeProv {
        let key = (cfg["api_key"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !key.isEmpty { return key }
    }
    if prov == "gemini" {
        let gem = (cfg["gemini_api_key"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !gem.isEmpty { return gem }
    }
    for envName in kProviderDefaults[prov]?.envVars ?? [] {
        if envName == "YOMI_API_KEY" && prov != activeProv { continue }
        let val = (ProcessInfo.processInfo.environment[envName] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !val.isEmpty { return val }
    }
    return ""
}

func loadAppConfig(dataDir: URL) -> [String: Any] {
    let configURL = dataDir.appendingPathComponent("config.json")
    var cfg: [String: Any] = [
        "onboarding_done": false,
        "ui_lang": "fr",
        "theme": "system",
        "mascot": "mochi",
        "engine_mode": "local",
        "default_model": "ollama:gemma4:12b",
        "api_provider": "gemini",
        "api_key": "",
        "api_base_url": "",
        "api_model": "gemini-3.8-flash",
        "source_lang": "en",
        "target_lang": "fr",
        "vision_mode": "auto",
        "gemini_api_key": "",
        "userscripts_dir": "",
    ]
    if let data = try? Data(contentsOf: configURL),
       let parsed = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
        for (k, v) in parsed { cfg[k] = v }
    }
    let prov = ((cfg["api_provider"] as? String) ?? "gemini").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    cfg["api_provider"] = kProviderDefaults[prov] != nil ? prov : "gemini"
    return cfg
}

func sanitizePublicConfig(_ cfg: [String: Any]) -> [String: Any] {
    var out = cfg
    let effective = effectiveCloudApiKey(cfg: cfg)
    out.removeValue(forKey: "gemini_api_key")
    out.removeValue(forKey: "api_key")
    out["has_api_key"] = !effective.isEmpty
    out["api_key_masked"] = maskKey(effective)
    return out
}

func testCloudApiKey(provider rawProvider: String, key rawKey: String, baseUrl rawBaseUrl: String = "", model rawModel: String = "") -> (Bool, String) {
    let provLower = rawProvider.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard let info = kProviderDefaults[provLower] else {
        return (false, "Fournisseur inconnu: \(provLower)")
    }
    let key = rawKey.trimmingCharacters(in: .whitespacesAndNewlines)
    if key.isEmpty {
        return (false, "Clé API vide / Empty API key")
    }
    var base = rawBaseUrl.trimmingCharacters(in: .whitespacesAndNewlines)
    if base.isEmpty { base = info.baseURL }
    while base.hasSuffix("/") { base.removeLast() }
    guard let parsedUrl = URL(string: base),
          let scheme = parsedUrl.scheme?.lowercased(),
          (scheme == "http" || scheme == "https"),
          parsedUrl.host != nil else {
        return (false, "URL de base invalide ou manquante / Invalid base URL")
    }

    if provLower == "gemini" {
        let (status, _, errTxt) = httpJSONSync(
            urlString: "\(base)/models?pageSize=1",
            method: "GET",
            headers: ["x-goog-api-key": key],
            timeout: 12.0
        )
        return (status >= 200 && status < 300) ? (true, "OK") : (false, "HTTP \(status): \(errTxt)")
    }

    if provLower == "anthropic" {
        let headers = [
            "x-api-key": key,
            "anthropic-version": "2023-06-01",
        ]
        let (status, _, errTxt) = httpJSONSync(
            urlString: "\(base)/models?limit=1",
            method: "GET",
            headers: headers,
            timeout: 12.0
        )
        if status >= 200 && status < 300 { return (true, "OK") }
        if status == 404 || status == 405 {
            let probeModel = rawModel.trimmingCharacters(in: .whitespacesAndNewlines)
            let body: [String: Any] = [
                "model": probeModel.isEmpty ? info.defaultModel : probeModel,
                "max_tokens": 1,
                "messages": [["role": "user", "content": "hi"]],
            ]
            let (st2, _, err2) = httpJSONSync(urlString: "\(base)/messages", method: "POST", body: body, headers: headers, timeout: 12.0)
            return (st2 >= 200 && st2 < 300) ? (true, "OK") : (false, "HTTP \(st2): \(err2)")
        }
        return (false, "HTTP \(status): \(errTxt)")
    }

    var headers = ["Authorization": "Bearer \(key)"]
    var checkUrl = "\(base)/models"
    if provLower == "openrouter" {
        headers["HTTP-Referer"] = "https://github.com/yomi"
        headers["X-Title"] = "Yomi"
        checkUrl = "\(base)/auth/key"
    }
    let (status, _, errTxt) = httpJSONSync(urlString: checkUrl, method: "GET", headers: headers, timeout: 12.0)
    if status >= 200 && status < 300 { return (true, "OK") }
    if status == 404 || status == 405 {
        let probeModel = rawModel.trimmingCharacters(in: .whitespacesAndNewlines)
        let body: [String: Any] = [
            "model": probeModel.isEmpty ? (info.defaultModel.isEmpty ? "gpt-4o-mini" : info.defaultModel) : probeModel,
            "max_tokens": 1,
            "messages": [["role": "user", "content": "hi"]],
        ]
        let (st2, _, err2) = httpJSONSync(urlString: "\(base)/chat/completions", method: "POST", body: body, headers: headers, timeout: 12.0)
        return (st2 >= 200 && st2 < 300) ? (true, "OK") : (false, "HTTP \(st2): \(err2)")
    }
    return (false, "HTTP \(status): \(errTxt)")
}

func recommendLocalModel(chip: String, ramGB: Int) -> String {
    var gen = 1
    if let regex = try? NSRegularExpression(pattern: #"\bM(\d+)\b"#, options: [.caseInsensitive]),
       let match = regex.firstMatch(in: chip, range: NSRange(chip.startIndex..., in: chip)),
       let range = Range(match.range(at: 1), in: chip),
       let parsed = Int(chip[range]) {
        gen = parsed
    }
    let isProPlus = chip.localizedCaseInsensitiveContains("pro")
        || chip.localizedCaseInsensitiveContains("max")
        || chip.localizedCaseInsensitiveContains("ultra")

    if ramGB < 16 {
        return "gemma4:e4b"
    }
    if ramGB < 24 {
        return (gen >= 3 || isProPlus) ? "gemma4:12b" : "gemma4:e4b"
    }
    return (ramGB >= 32 || gen >= 3 || isProPlus) ? "gemma4:26b" : "gemma4:12b"
}

func detectUserscriptsInfo(cfg: [String: Any]) -> [String: Any] {
    let fm = FileManager.default
    let home = fm.homeDirectoryForCurrentUser.path
    let appInstalled = fm.fileExists(atPath: "/Applications/Userscripts.app")
        || fm.fileExists(atPath: "\(home)/Applications/Userscripts.app")
        || NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.userscripts.macos.Userscripts") != nil

    var candidates: [String] = []
    if let saved = (cfg["userscripts_dir"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !saved.isEmpty {
        candidates.append(saved)
    }
    candidates.append(contentsOf: [
        "\(home)/Library/Containers/com.userscripts.macos.Userscripts-Extension/Data/Documents/scripts",
        "\(home)/Library/Containers/com.userscripts.macos/Data/Documents/scripts",
        "\(home)/Documents/Userscripts",
        "\(home)/Userscripts",
    ])

    var detectedDir = ""
    for dir in candidates {
        var isDir: ObjCBool = false
        if fm.fileExists(atPath: dir, isDirectory: &isDir), isDir.boolValue, fm.isWritableFile(atPath: dir) {
            detectedDir = dir
            break
        }
    }
    let scriptPath = detectedDir.isEmpty ? "" : "\(detectedDir)/yomi.user.js"
    let scriptInstalled = !scriptPath.isEmpty && fm.fileExists(atPath: scriptPath)

    return [
        "app_installed": appInstalled,
        "detected_dir": detectedDir,
        "script_installed": scriptInstalled,
        "script_path": scriptPath,
    ]
}

func inspectSystemState() -> [String: Any] {
    let dataDir = appSupportDataDir()
    let arch = hostArchitecture()
    let chip = sysctlString("machdep.cpu.brand_string")
    let memBytes = sysctlUInt64("hw.memsize")
    let ramGB = max(1, Int((Double(memBytes) / 1_073_741_824.0).rounded()))
    let recModel = recommendLocalModel(chip: chip, ramGB: ramGB)

    let py = detectPython(dataDir: dataDir)
    let ollama = detectOllama()
    let ocrReady = detectOCRReady()
    let (srvStatus, healthJson, _) = httpJSONSync(urlString: "\(kServerBaseURL)/api/health", timeout: 1.0)
    let rawConfig = loadAppConfig(dataDir: dataDir)
    let userscriptsInfo = detectUserscriptsInfo(cfg: rawConfig)

    return [
        "arch": arch,
        "apple_silicon": arch == "arm64",
        "chip": chip.isEmpty ? "Apple Silicon" : chip,
        "ram_gb": ramGB,
        "macos": ProcessInfo.processInfo.operatingSystemVersionString,
        "recommended_model": recModel,
        "python": py,
        "ollama": ollama,
        "ocr_ready": ocrReady,
        "userscripts": userscriptsInfo,
        "server_running": srvStatus == 200,
        "server_health": healthJson ?? [:],
        "config": sanitizePublicConfig(rawConfig),
        "data_dir": dataDir.path,
    ]
}

final class DownloadHelper: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    private let onProgress: (Int64, Int64) -> Void
    private let onComplete: (Result<URL, Error>) -> Void
    private var session: URLSession?
    private var lastReportTime: TimeInterval = 0

    init(onProgress: @escaping (Int64, Int64) -> Void, onComplete: @escaping (Result<URL, Error>) -> Void) {
        self.onProgress = onProgress
        self.onComplete = onComplete
        super.init()
    }

    func start(url: URL, dest: URL) {
        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 60
        cfg.timeoutIntervalForResource = 1800
        let sess = URLSession(configuration: cfg, delegate: self, delegateQueue: nil)
        self.session = sess
        let task = sess.downloadTask(with: url)
        objc_setAssociatedObject(task, "destURL", dest, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
        task.resume()
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        let now = Date().timeIntervalSince1970
        if now - lastReportTime >= 0.25 || (totalBytesExpectedToWrite > 0 && totalBytesWritten == totalBytesExpectedToWrite) {
            lastReportTime = now
            onProgress(totalBytesWritten, totalBytesExpectedToWrite)
        }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        let dest = (objc_getAssociatedObject(downloadTask, "destURL") as? URL)
            ?? FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        do {
            let fm = FileManager.default
            if fm.fileExists(atPath: dest.path) { try fm.removeItem(at: dest) }
            try fm.moveItem(at: location, to: dest)
            if let http = downloadTask.response as? HTTPURLResponse, http.statusCode >= 400 {
                throw NSError(domain: "HTTP", code: http.statusCode, userInfo: [NSLocalizedDescriptionKey: "HTTP \(http.statusCode)"])
            }
            onComplete(.success(dest))
        } catch {
            onComplete(.failure(error))
        }
        self.session?.finishTasksAndInvalidate()
        self.session = nil
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let err = error {
            onComplete(.failure(err))
            self.session?.finishTasksAndInvalidate()
            self.session = nil
        }
    }
}

final class PullStreamHelper: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private var session: URLSession?
    private var task: URLSessionDataTask?
    private var buffer = Data()
    private let onLine: ([String: Any]) -> Void
    private let onFinish: (Result<Void, Error>) -> Void

    init(onLine: @escaping ([String: Any]) -> Void, onFinish: @escaping (Result<Void, Error>) -> Void) {
        self.onLine = onLine
        self.onFinish = onFinish
        super.init()
    }

    func start(model: String) {
        guard let url = URL(string: "\(kOllamaBaseURL)/api/pull") else {
            onFinish(.failure(NSError(domain: "Pull", code: 1, userInfo: [NSLocalizedDescriptionKey: "Invalid Ollama URL"])))
            return
        }
        var req = URLRequest(url: url, timeoutInterval: 3600)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: ["name": model, "stream": true])

        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 120
        cfg.timeoutIntervalForResource = 7200
        let sess = URLSession(configuration: cfg, delegate: self, delegateQueue: nil)
        self.session = sess
        let t = sess.dataTask(with: req)
        self.task = t
        t.resume()
    }

    func cancel() {
        task?.cancel()
        session?.invalidateAndCancel()
        session = nil
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        buffer.append(data)
        while let nl = buffer.firstIndex(of: 0x0A) {
            let lineData = buffer.subdata(in: 0..<nl)
            buffer.removeSubrange(0...nl)
            if let parsed = try? JSONSerialization.jsonObject(with: lineData) as? [String: Any] {
                onLine(parsed)
            }
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if !buffer.isEmpty, let parsed = try? JSONSerialization.jsonObject(with: buffer) as? [String: Any] {
            onLine(parsed)
        }
        if let err = error {
            onFinish(.failure(err))
        } else if let http = task.response as? HTTPURLResponse, http.statusCode >= 400 {
            onFinish(.failure(NSError(domain: "HTTP", code: http.statusCode, userInfo: [NSLocalizedDescriptionKey: "HTTP \(http.statusCode)"])))
        } else {
            onFinish(.success(()))
        }
        self.session?.finishTasksAndInvalidate()
        self.session = nil
    }
}

final class RuntimeBox: @unchecked Sendable {
    private let lock = NSLock()
    var serverProcess: Process?
    var ollamaProcess: Process?
    var activePull: PullStreamHelper?
    var activeDownload: DownloadHelper?

    func withLock<T>(_ body: (RuntimeBox) -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body(self)
    }
}

@MainActor
final class StatusMenuPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

@MainActor
final class StatusMenuRowButton: NSView {
    private let titleLabel = NSTextField(labelWithString: "")
    private let shortcutLabel = NSTextField(labelWithString: "")
    private let hoverColor: NSColor
    private var trackingArea: NSTrackingArea?
    private var isHovered = false
    private let onClick: () -> Void

    init(
        title: String,
        shortcut: String = "",
        textColor: NSColor,
        mutedColor: NSColor,
        hoverColor: NSColor,
        onClick: @escaping () -> Void
    ) {
        self.hoverColor = hoverColor
        self.onClick = onClick
        super.init(frame: NSRect(x: 0, y: 0, width: 236, height: 28))
        wantsLayer = true
        layer?.cornerRadius = 7

        titleLabel.stringValue = title
        titleLabel.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        titleLabel.textColor = textColor
        titleLabel.lineBreakMode = .byTruncatingTail
        titleLabel.translatesAutoresizingMaskIntoConstraints = false

        shortcutLabel.stringValue = shortcut
        shortcutLabel.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .semibold)
        shortcutLabel.textColor = mutedColor
        shortcutLabel.alignment = .right
        shortcutLabel.isHidden = shortcut.isEmpty
        shortcutLabel.translatesAutoresizingMaskIntoConstraints = false

        addSubview(titleLabel)
        addSubview(shortcutLabel)

        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: 28),
            titleLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            titleLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            shortcutLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            shortcutLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            titleLabel.trailingAnchor.constraint(lessThanOrEqualTo: shortcutLabel.leadingAnchor, constant: -8),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool {
        true
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let existing = trackingArea {
            removeTrackingArea(existing)
        }
        let area = NSTrackingArea(
            rect: bounds,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        trackingArea = area
    }

    override func mouseEntered(with event: NSEvent) {
        isHovered = true
        layer?.backgroundColor = hoverColor.cgColor
    }

    override func mouseExited(with event: NSEvent) {
        isHovered = false
        layer?.backgroundColor = NSColor.clear.cgColor
    }

    override func mouseDown(with event: NSEvent) {
        layer?.backgroundColor = hoverColor.withAlphaComponent(min(1.0, hoverColor.alphaComponent + 0.15)).cgColor
    }

    override func mouseUp(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        layer?.backgroundColor = (isHovered && bounds.contains(pt) ? hoverColor : NSColor.clear).cgColor
        if bounds.contains(pt) {
            onClick()
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, WKScriptMessageHandler {
    private var statusItem: NSStatusItem!
    private var window: NSWindow!
    private var webView: WKWebView!
    private var statusPanel: StatusMenuPanel?
    private var statusPanelLineLabel: NSTextField?
    private var statusPanelDotView: NSView?
    private var statusGlobalMonitor: Any?
    private var statusLocalMonitor: Any?
    private var lastServerRunning = false

    private let runtime = RuntimeBox()
    private var isQuitting = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        if hostArchitecture() != "arm64" {
            let alert = NSAlert()
            alert.alertStyle = .critical
            alert.messageText = "Apple Silicon requis / Apple Silicon Required"
            alert.informativeText = "Yomi nécessite un Mac Apple Silicon (M1 / M2 / M3 / M4 / M5)."
            alert.runModal()
            NSApp.terminate(nil)
            return
        }

        NSApp.setActivationPolicy(.regular)
        setupMainMenu()
        setupMenuBar()
        setupWindow()

        let dataDir = appSupportDataDir()
        try? FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
        let cfg = loadAppConfig(dataDir: dataDir)
        let onboardingDone = (cfg["onboarding_done"] as? Bool) ?? false

        if onboardingDone {
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                self?.startBackendIfPossible { _ in }
            }
        }
        DispatchQueue.global(qos: .utility).async { [weak self] in
            self?.syncUserscriptIfConfigured()
        }
        showMainWindow()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showMainWindow()
        return true
    }

    func applicationWillTerminate(_ notification: Notification) {
        closeStatusMenuPanel()
        shutdownBackgroundProcesses()
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if !isQuitting {
            sender.orderOut(nil)
            NSApp.setActivationPolicy(.accessory)
            return false
        }
        return true
    }

    func windowDidBecomeKey(_ notification: Notification) {
        if NSApp.activationPolicy() != .regular {
            NSApp.setActivationPolicy(.regular)
        }
    }

    func windowWillEnterFullScreen(_ notification: Notification) {
        pushEvent(name: "fullscreen", payload: ["active": true])
    }

    func windowDidEnterFullScreen(_ notification: Notification) {
        pushEvent(name: "fullscreen", payload: ["active": true])
    }

    func windowWillExitFullScreen(_ notification: Notification) {
        pushEvent(name: "fullscreen", payload: ["active": false])
    }

    func windowDidExitFullScreen(_ notification: Notification) {
        pushEvent(name: "fullscreen", payload: ["active": false])
    }

    private func setupMainMenu() {
        let mainMenu = NSMenu()

        let appMenuItem = NSMenuItem()
        mainMenu.addItem(appMenuItem)
        let appMenu = NSMenu()
        let settingsItem = NSMenuItem(title: "Réglages...", action: #selector(menuOpenSettings), keyEquivalent: ",")
        settingsItem.target = self
        appMenu.addItem(settingsItem)
        let homeItem = NSMenuItem(title: "Accueil", action: #selector(menuOpenHome), keyEquivalent: "o")
        homeItem.target = self
        appMenu.addItem(homeItem)
        appMenu.addItem(.separator())
        let quitItem = NSMenuItem(title: "Quitter Yomi", action: #selector(menuQuit), keyEquivalent: "q")
        quitItem.target = self
        appMenu.addItem(quitItem)
        appMenuItem.submenu = appMenu

        let editMenuItem = NSMenuItem()
        mainMenu.addItem(editMenuItem)
        let editMenu = NSMenu(title: "Édition")
        editMenu.addItem(withTitle: "Couper", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copier", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Coller", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Tout sélectionner", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenuItem.submenu = editMenu

        NSApp.mainMenu = mainMenu
    }

func makeMenuBarCatIcon(awake: Bool) -> NSImage {
    let grid: [String] = awake
        ? [
            "..##........##..",
            ".####.####.####.",
            ".##############.",
            ".##############.",
            ".###..####..###.",
            ".###..####..###.",
            ".##############.",
            ".##############.",
            "..############..",
            "..###########.#.",
            "..#############.",
            "..############..",
            "..####....####..",
            "...##......##...",
        ]
        : [
            "................",
            "................",
            "................",
            "..##........##..",
            ".####.####.####.",
            ".##############.",
            ".##############.",
            ".##...####...##.",
            ".##############.",
            ".##############.",
            ".##############.",
            "..############..",
            "...##########...",
            "................",
        ]

    let scale = 2
    let w = 16 * scale
    let h = 14 * scale
    let image = NSImage(size: NSSize(width: 18, height: 16))
    if let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: w,
        pixelsHigh: h,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: w * 4,
        bitsPerPixel: 32
    ), let ptr = rep.bitmapData {
        memset(ptr, 0, w * h * 4)
        for (gy, row) in grid.enumerated() {
            for (gx, ch) in row.enumerated() where ch == "#" {
                for dy in 0..<scale {
                    for dx in 0..<scale {
                        let px = gx * scale + dx
                        let py = gy * scale + dy
                        let idx = (py * w + px) * 4
                        ptr[idx] = 0
                        ptr[idx + 1] = 0
                        ptr[idx + 2] = 0
                        ptr[idx + 3] = 255
                    }
                }
            }
        }
        image.addRepresentation(rep)
    }
    image.isTemplate = true
    return image
}

    private func setupMenuBar() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let btn = statusItem.button {
            btn.title = ""
            btn.image = makeMenuBarCatIcon(awake: false)
            btn.toolTip = "Yomi"
            btn.target = self
            btn.action = #selector(statusItemClicked(_:))
            btn.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
    }

    @objc private func statusItemClicked(_ sender: Any?) {
        if let panel = statusPanel, panel.isVisible {
            closeStatusMenuPanel()
            return
        }
        showStatusMenuPanel()
    }

    private func isDarkMenuAppearance() -> Bool {
        let theme = ((loadAppConfig(dataDir: appSupportDataDir())["theme"] as? String) ?? "system").lowercased()
        if theme == "dark" { return true }
        if theme == "light" { return false }
        return NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    }

    private func closeStatusMenuPanel() {
        if let gm = statusGlobalMonitor {
            NSEvent.removeMonitor(gm)
            statusGlobalMonitor = nil
        }
        if let lm = statusLocalMonitor {
            NSEvent.removeMonitor(lm)
            statusLocalMonitor = nil
        }
        statusPanel?.orderOut(nil)
        statusPanel = nil
        statusPanelLineLabel = nil
        statusPanelDotView = nil
    }

    private func showStatusMenuPanel() {
        // When another app is active / fullscreen, switching to .accessory lets the floating
        // panel appear directly above the foreign fullscreen Space without switching Spaces.
        if !NSApp.isActive, !window.styleMask.contains(.fullScreen), NSApp.activationPolicy() != .accessory {
            NSApp.setActivationPolicy(.accessory)
            DispatchQueue.main.async { [weak self] in
                self?.presentStatusMenuPanel()
            }
            return
        }
        presentStatusMenuPanel()
    }

    private func presentStatusMenuPanel() {
        closeStatusMenuPanel()

        let dark = isDarkMenuAppearance()
        let panelBg = dark
            ? NSColor(srgbRed: 0.125, green: 0.114, blue: 0.094, alpha: 0.98)
            : NSColor(srgbRed: 0.980, green: 0.961, blue: 0.918, alpha: 0.98)
        let borderCol = dark
            ? NSColor(srgbRed: 0.965, green: 0.831, blue: 0.545, alpha: 0.22)
            : NSColor(srgbRed: 0.875, green: 0.824, blue: 0.729, alpha: 1.0)
        let statusBg = dark
            ? NSColor(srgbRed: 0.180, green: 0.161, blue: 0.133, alpha: 1.0)
            : NSColor(srgbRed: 0.957, green: 0.914, blue: 0.827, alpha: 1.0)
        let textCol = dark
            ? NSColor(srgbRed: 0.965, green: 0.937, blue: 0.886, alpha: 1.0)
            : NSColor(srgbRed: 0.137, green: 0.118, blue: 0.102, alpha: 1.0)
        let mutedCol = dark
            ? NSColor(srgbRed: 0.663, green: 0.616, blue: 0.557, alpha: 1.0)
            : NSColor(srgbRed: 0.471, green: 0.427, blue: 0.380, alpha: 1.0)
        let hoverCol = dark
            ? NSColor(srgbRed: 0.949, green: 0.792, blue: 0.455, alpha: 0.24)
            : NSColor(srgbRed: 0.961, green: 0.808, blue: 0.459, alpha: 0.52)
        let greenCol = dark
            ? NSColor(srgbRed: 0.498, green: 0.690, blue: 0.412, alpha: 1.0)
            : NSColor(srgbRed: 0.310, green: 0.498, blue: 0.231, alpha: 1.0)
        let redCol = dark
            ? NSColor(srgbRed: 0.898, green: 0.420, blue: 0.388, alpha: 1.0)
            : NSColor(srgbRed: 0.722, green: 0.227, blue: 0.196, alpha: 1.0)
        let sepCol = dark
            ? NSColor(srgbRed: 0.965, green: 0.831, blue: 0.545, alpha: 0.12)
            : NSColor(srgbRed: 0.137, green: 0.118, blue: 0.102, alpha: 0.08)

        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 3
        stack.edgeInsets = NSEdgeInsets(top: 8, left: 8, bottom: 8, right: 8)
        stack.translatesAutoresizingMaskIntoConstraints = false

        let statusPill = NSView(frame: NSRect(x: 0, y: 0, width: 236, height: 28))
        statusPill.wantsLayer = true
        statusPill.layer?.cornerRadius = 8
        statusPill.layer?.backgroundColor = statusBg.cgColor
        statusPill.translatesAutoresizingMaskIntoConstraints = false

        let dot = NSView(frame: NSRect(x: 0, y: 0, width: 8, height: 8))
        dot.wantsLayer = true
        dot.layer?.cornerRadius = 4
        dot.layer?.backgroundColor = (lastServerRunning ? greenCol : mutedCol).cgColor
        dot.translatesAutoresizingMaskIntoConstraints = false

        let statusLbl = NSTextField(labelWithString: lastServerRunning ? "Serveur local actif (port 8765)" : "Serveur arrêté")
        statusLbl.font = NSFont.systemFont(ofSize: 11.5, weight: .semibold)
        statusLbl.textColor = lastServerRunning ? greenCol : mutedCol
        statusLbl.lineBreakMode = .byTruncatingTail
        statusLbl.translatesAutoresizingMaskIntoConstraints = false

        statusPill.addSubview(dot)
        statusPill.addSubview(statusLbl)
        NSLayoutConstraint.activate([
            statusPill.widthAnchor.constraint(equalToConstant: 236),
            statusPill.heightAnchor.constraint(equalToConstant: 28),
            dot.leadingAnchor.constraint(equalTo: statusPill.leadingAnchor, constant: 10),
            dot.centerYAnchor.constraint(equalTo: statusPill.centerYAnchor),
            dot.widthAnchor.constraint(equalToConstant: 8),
            dot.heightAnchor.constraint(equalToConstant: 8),
            statusLbl.leadingAnchor.constraint(equalTo: dot.trailingAnchor, constant: 8),
            statusLbl.trailingAnchor.constraint(equalTo: statusPill.trailingAnchor, constant: -10),
            statusLbl.centerYAnchor.constraint(equalTo: statusPill.centerYAnchor),
        ])
        statusPanelLineLabel = statusLbl
        statusPanelDotView = dot

        func makeSep() -> NSView {
            let wrap = NSView(frame: NSRect(x: 0, y: 0, width: 236, height: 7))
            wrap.translatesAutoresizingMaskIntoConstraints = false
            let line = NSView()
            line.wantsLayer = true
            line.layer?.backgroundColor = sepCol.cgColor
            line.translatesAutoresizingMaskIntoConstraints = false
            wrap.addSubview(line)
            NSLayoutConstraint.activate([
                wrap.widthAnchor.constraint(equalToConstant: 236),
                wrap.heightAnchor.constraint(equalToConstant: 7),
                line.leadingAnchor.constraint(equalTo: wrap.leadingAnchor, constant: 6),
                line.trailingAnchor.constraint(equalTo: wrap.trailingAnchor, constant: -6),
                line.centerYAnchor.constraint(equalTo: wrap.centerYAnchor),
                line.heightAnchor.constraint(equalToConstant: 1),
            ])
            return wrap
        }

        func addRow(_ title: String, shortcut: String = "", danger: Bool = false, action: @escaping () -> Void) {
            let row = StatusMenuRowButton(
                title: title,
                shortcut: shortcut,
                textColor: danger ? redCol : textCol,
                mutedColor: mutedCol,
                hoverColor: hoverCol
            ) { [weak self] in
                self?.closeStatusMenuPanel()
                action()
            }
            row.translatesAutoresizingMaskIntoConstraints = false
            row.widthAnchor.constraint(equalToConstant: 236).isActive = true
            stack.addArrangedSubview(row)
        }

        stack.addArrangedSubview(statusPill)
        stack.addArrangedSubview(makeSep())
        addRow("Ouvrir / Accueil", shortcut: "⌘O") { [weak self] in self?.menuOpenHome() }
        addRow("Yomi Studio") { [weak self] in self?.menuOpenTestPage() }
        addRow("Réglages...", shortcut: "⌘,") { [weak self] in self?.menuOpenSettings() }
        addRow("Relancer l'assistant...") { [weak self] in self?.menuOpenWizard() }
        stack.addArrangedSubview(makeSep())
        addRow("Libérer la RAM Ollama") { [weak self] in self?.menuUnloadRAM() }
        addRow("Vider le cache de traduction") { [weak self] in self?.menuClearCache() }
        stack.addArrangedSubview(makeSep())
        addRow("Quitter Yomi", shortcut: "⌘Q", danger: true) { [weak self] in self?.menuQuit() }

        let container = NSView()
        container.wantsLayer = true
        container.layer?.backgroundColor = panelBg.cgColor
        container.layer?.cornerRadius = 14
        container.layer?.borderWidth = 1
        container.layer?.borderColor = borderCol.cgColor
        container.layer?.masksToBounds = true
        container.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: container.topAnchor),
            stack.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: container.trailingAnchor),
        ])
        container.layoutSubtreeIfNeeded()
        let size = container.fittingSize

        let panel = StatusMenuPanel(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.popUpMenuWindow)))
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.contentView = container

        let anchorRect: NSRect = statusItem?.button?.window?.frame
            ?? NSRect(origin: NSEvent.mouseLocation, size: NSSize(width: 24, height: 24))
        let screen = NSScreen.screens.first(where: { $0.frame.contains(anchorRect.origin) })
            ?? NSScreen.main
            ?? NSScreen.screens.first
        var x = anchorRect.minX
        var y = anchorRect.minY - size.height - 6
        if let vf = screen?.frame {
            x = max(vf.minX + 8, min(x, vf.maxX - size.width - 8))
            y = max(vf.minY + 8, min(y, vf.maxY - size.height - 6))
        }
        panel.setFrameOrigin(NSPoint(x: x, y: y))
        panel.orderFrontRegardless()
        statusPanel = panel

        statusGlobalMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.closeStatusMenuPanel()
            }
        }
        statusLocalMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .keyDown]) { [weak self] event in
            guard let self else { return event }
            if event.type == .keyDown {
                if event.keyCode == 53 {
                    self.closeStatusMenuPanel()
                    return nil
                }
                return event
            }
            if let p = self.statusPanel, p.isVisible {
                let inPanel = (event.window === p)
                let inBtn = (event.window === self.statusItem?.button?.window)
                if !inPanel && !inBtn {
                    self.closeStatusMenuPanel()
                }
            }
            return event
        }
    }

    private func setupWindow() {
        let contentController = WKUserContentController()
        contentController.add(self, name: "bridge")

        let wkConfig = WKWebViewConfiguration()
        wkConfig.userContentController = contentController

        let visible = (NSScreen.main ?? NSScreen.screens.first)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1280, height: 800)
        let initW = max(780.0, min(960.0, Double(visible.width) * 0.62))
        let initH = max(580.0, min(680.0, Double(visible.height) * 0.72))
        let rect = NSRect(x: 0, y: 0, width: initW, height: initH)
        window = NSWindow(
            contentRect: rect,
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Yomi"
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true
        window.isOpaque = false
        window.backgroundColor = .clear
        window.minSize = NSSize(width: 680, height: 500)
        window.collectionBehavior.insert(.fullScreenPrimary)
        window.center()
        window.delegate = self
        window.isReleasedWhenClosed = false
        let initialTheme = (loadAppConfig(dataDir: appSupportDataDir())["theme"] as? String) ?? "system"
        applyWindowTheme(initialTheme)

        let visualEffect = NSVisualEffectView(frame: rect)
        visualEffect.material = .underWindowBackground
        visualEffect.blendingMode = .behindWindow
        visualEffect.state = .followsWindowActiveState
        visualEffect.autoresizingMask = [.width, .height]
        window.contentView = visualEffect

        webView = WKWebView(frame: rect, configuration: wkConfig)
        webView.translatesAutoresizingMaskIntoConstraints = false
        webView.setValue(false, forKey: "drawsBackground")
        visualEffect.addSubview(webView)
        NSLayoutConstraint.activate([
            webView.topAnchor.constraint(equalTo: visualEffect.topAnchor),
            webView.bottomAnchor.constraint(equalTo: visualEffect.bottomAnchor),
            webView.leadingAnchor.constraint(equalTo: visualEffect.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: visualEffect.trailingAnchor),
        ])

        if let uiURL = resolveResourceURL("ui/app.html") ?? resolveResourceURL("app/ui/app.html") {
            webView.loadFileURL(uiURL, allowingReadAccessTo: uiURL.deletingLastPathComponent())
        } else {
            webView.loadHTMLString("<html><body style='background:transparent;font-family:-apple-system;padding:32px'><h2>UI manquante (ui/app.html)</h2></body></html>", baseURL: nil)
        }
    }

    private func showMainWindow() {
        closeStatusMenuPanel()
        NSApp.setActivationPolicy(.regular)
        if window.isMiniaturized {
            window.deminiaturize(nil)
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func refreshMenuStatus(_ running: Bool) {
        lastServerRunning = running
        statusItem?.button?.image = makeMenuBarCatIcon(awake: running)
        if let lbl = statusPanelLineLabel {
            let dark = isDarkMenuAppearance()
            let activeCol = dark
                ? NSColor(srgbRed: 0.498, green: 0.690, blue: 0.412, alpha: 1.0)
                : NSColor(srgbRed: 0.310, green: 0.498, blue: 0.231, alpha: 1.0)
            let mutedCol = dark
                ? NSColor(srgbRed: 0.663, green: 0.616, blue: 0.557, alpha: 1.0)
                : NSColor(srgbRed: 0.471, green: 0.427, blue: 0.380, alpha: 1.0)
            lbl.stringValue = running ? "Serveur local actif (port 8765)" : "Serveur arrêté"
            lbl.textColor = running ? activeCol : mutedCol
            statusPanelDotView?.layer?.backgroundColor = (running ? activeCol : mutedCol).cgColor
        }
    }

    private func applyWindowTheme(_ rawTheme: String) {
        guard let window = window else { return }
        switch rawTheme.lowercased() {
        case "light":
            window.appearance = NSAppearance(named: .aqua)
        case "dark":
            window.appearance = NSAppearance(named: .darkAqua)
        default:
            window.appearance = nil
        }
        window.backgroundColor = .clear
    }

    @objc private func menuOpenHome() {
        showMainWindow()
        pushEvent(name: "navigate", payload: ["view": "home"])
    }

    @objc private func menuOpenSettings() {
        showMainWindow()
        pushEvent(name: "navigate", payload: ["view": "settings"])
    }

    @objc private func menuOpenWizard() {
        showMainWindow()
        pushEvent(name: "navigate", payload: ["view": "wizard"])
    }

    @objc private func menuOpenTestPage() {
        showMainWindow()
        pushEvent(name: "navigate", payload: ["view": "test"])
    }

    @objc private func menuUnloadRAM() {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            _ = self?.performUnloadRAM()
        }
    }

    @objc private func menuClearCache() {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            _ = self?.performClearCache()
        }
    }

    @objc private func menuQuit() {
        isQuitting = true
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.shutdownBackgroundProcesses()
            Task { @MainActor in
                NSApp.terminate(nil)
            }
        }
    }

    nonisolated private func evalJS(_ script: String) {
        Task { @MainActor [weak self] in
            _ = try? await self?.webView?.evaluateJavaScript(script)
        }
    }

    nonisolated private func encodeJSON(_ obj: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: obj),
              let str = String(data: data, encoding: .utf8) else {
            return "{}"
        }
        return str
    }

    nonisolated private func pushEvent(name: String, payload: [String: Any]) {
        let nameJSON = encodeJSON(["event": name, "payload": payload])
        evalJS("window.__smtBridgeEvent && window.__smtBridgeEvent(\(nameJSON));")
    }

    nonisolated private func resolveBridgeCall(id: String, ok: Bool, payload: Any) {
        let wrapped = encodeJSON(["id": id, "ok": ok, "data": payload])
        evalJS("window.__smtBridgeResolve && window.__smtBridgeResolve(\(wrapped));")
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any],
              let callId = body["id"] as? String,
              let action = body["action"] as? String else {
            return
        }
        let payload = (body["payload"] as? [String: Any]) ?? [:]
        nonisolated(unsafe) let sendablePayload = payload
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.handleBridgeAction(callId: callId, action: action, payload: sendablePayload)
        }
    }

    nonisolated private func handleBridgeAction(callId: String, action: String, payload: [String: Any]) {
        switch action {
        case "getState":
            let state = inspectSystemState()
            let running = (state["server_running"] as? Bool) ?? false
            Task { @MainActor [weak self] in
                self?.refreshMenuStatus(running)
            }
            resolveBridgeCall(id: callId, ok: true, payload: state)

        case "installPython":
            handleInstallPython(callId: callId)

        case "installOllama":
            handleInstallOllama(callId: callId)

        case "startOllama":
            let ok = ensureOllamaRunning()
            resolveBridgeCall(id: callId, ok: ok, payload: inspectSystemState())

        case "pullModel":
            let modelName = (payload["model"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if modelName.isEmpty {
                resolveBridgeCall(id: callId, ok: false, payload: ["error": "Model required"])
                return
            }
            handlePullModel(callId: callId, modelName: modelName)

        case "cancelPull":
            runtime.withLock { box in
                box.activePull?.cancel()
                box.activePull = nil
            }
            resolveBridgeCall(id: callId, ok: true, payload: ["cancelled": true])

        case "testGeminiKey", "testApiKey":
            let cfg = loadAppConfig(dataDir: appSupportDataDir())
            let prov = ((payload["provider"] as? String) ?? (cfg["api_provider"] as? String) ?? "gemini")
                .trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            var key = (payload["key"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if key.isEmpty {
                key = effectiveCloudApiKey(cfg: cfg, provider: prov)
            }
            var baseUrl = (payload["base_url"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if baseUrl.isEmpty && prov == ((cfg["api_provider"] as? String) ?? "gemini").lowercased() {
                baseUrl = (cfg["api_base_url"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            }
            let model = (payload["model"] as? String ?? (cfg["api_model"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let (ok, detail) = testCloudApiKey(provider: prov, key: key, baseUrl: baseUrl, model: model)
            if ok {
                resolveBridgeCall(id: callId, ok: true, payload: ["ok": true])
            } else {
                resolveBridgeCall(id: callId, ok: false, payload: ["error": detail])
            }

        case "autoInstallSafariUserscript":
            let cfg = loadAppConfig(dataDir: appSupportDataDir())
            let info = detectUserscriptsInfo(cfg: cfg)
            let dir = (info["detected_dir"] as? String) ?? ""
            if !dir.isEmpty {
                do {
                    let dst = try installUserscriptFile(to: URL(fileURLWithPath: dir, isDirectory: true))
                    _ = persistConfigUpdates(["userscripts_dir": dir])
                    resolveBridgeCall(id: callId, ok: true, payload: ["auto": true, "path": dst.path, "state": inspectSystemState()])
                } catch {
                    resolveBridgeCall(id: callId, ok: false, payload: ["error": error.localizedDescription])
                }
            } else {
                Task { @MainActor [weak self] in
                    self?.handleInstallUserscriptPanel(callId: callId)
                }
            }

        case "openUserscriptInstallUrl":
            startBackendIfPossible(forceRestart: false) { [weak self] ok in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    if ok, let url = URL(string: "\(kServerBaseURL)/yomi.user.js") {
                        NSWorkspace.shared.open(url)
                        self.resolveBridgeCall(id: callId, ok: true, payload: ["opened": true, "url": url.absoluteString, "state": inspectSystemState()])
                    } else {
                        self.resolveBridgeCall(id: callId, ok: false, payload: ["error": "Serveur local indisponible (vérifiez Python)."])
                    }
                }
            }

        case "openUserscriptsAppStore":
            Task { @MainActor in
                if let url = URL(string: "macappstore://apps.apple.com/app/id1463298887") {
                    NSWorkspace.shared.open(url)
                }
            }
            resolveBridgeCall(id: callId, ok: true, payload: [:])

        case "installUserscriptToFolder":
            Task { @MainActor [weak self] in
                self?.handleInstallUserscriptPanel(callId: callId)
            }

        case "copyUserscript":
            if let srcURL = resolveResourceURL("userscript/yomi.user.js"),
               let txt = try? String(contentsOf: srcURL, encoding: .utf8) {
                Task { @MainActor in
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(txt, forType: .string)
                }
                resolveBridgeCall(id: callId, ok: true, payload: ["copied": true])
            } else {
                resolveBridgeCall(id: callId, ok: false, payload: ["error": "Userscript introuvable"])
            }

        case "quitApp":
            Task { @MainActor [weak self] in
                self?.menuQuit()
            }
            resolveBridgeCall(id: callId, ok: true, payload: [:])

        case "setTheme":
            let rawTheme = (payload["theme"] as? String ?? "system").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            let cleanTheme = ["light", "dark", "system"].contains(rawTheme) ? rawTheme : "system"
            _ = persistConfigUpdates(["theme": cleanTheme])
            resolveBridgeCall(id: callId, ok: true, payload: ["theme": cleanTheme])

        case "saveSettings":
            _ = persistConfigUpdates(payload)
            resolveBridgeCall(id: callId, ok: true, payload: inspectSystemState())

        case "saveConfigAndStartServer":
            _ = persistConfigUpdates(payload)
            startBackendIfPossible(forceRestart: true) { ok in
                self.resolveBridgeCall(id: callId, ok: ok, payload: inspectSystemState())
            }

        case "restartServer":
            startBackendIfPossible(forceRestart: true) { ok in
                self.resolveBridgeCall(id: callId, ok: ok, payload: inspectSystemState())
            }

        case "startServer":
            startBackendIfPossible(forceRestart: false) { ok in
                self.resolveBridgeCall(id: callId, ok: ok, payload: inspectSystemState())
            }

        case "stopServer":
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                guard let self else { return }
                self.stopExistingServer()
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.refreshMenuStatus(false)
                    self.resolveBridgeCall(id: callId, ok: true, payload: inspectSystemState())
                }
            }

        case "unloadRam":
            let unloaded = performUnloadRAM()
            resolveBridgeCall(id: callId, ok: true, payload: ["unloaded": unloaded])

        case "clearCache":
            let deleted = performClearCache()
            resolveBridgeCall(id: callId, ok: true, payload: ["deleted": deleted])

        case "openTestPage":
            Task { @MainActor [weak self] in self?.menuOpenTestPage() }
            resolveBridgeCall(id: callId, ok: true, payload: [:])

        case "pickTestImage":
            Task { @MainActor [weak self] in
                self?.handlePickTestImage(callId: callId)
            }

        case "readClipboardImage":
            Task { @MainActor [weak self] in
                guard let self else { return }
                if let hit = self.readClipboardImageDataURL() {
                    self.resolveBridgeCall(id: callId, ok: true, payload: [
                        "found": true,
                        "data_url": hit.dataUrl,
                        "filename": hit.filename,
                    ])
                } else {
                    self.resolveBridgeCall(id: callId, ok: true, payload: ["found": false])
                }
            }

        case "getTestHealth":
            let autoStart = (payload["auto_start"] as? Bool) ?? true
            handleGetTestHealth(callId: callId, autoStart: autoStart)

        case "translateTestImage":
            let imageB64 = (payload["image_base64"] as? String) ?? ""
            let model = (payload["model"] as? String) ?? ""
            let srcLang = (payload["source_lang"] as? String) ?? "en"
            let tgtLang = (payload["target_lang"] as? String) ?? "fr"
            let vision = (payload["vision"] as? String) ?? "auto"
            let skipCache = (payload["skip_cache"] as? Bool) ?? false
            handleTranslateTestImage(
                callId: callId,
                imageBase64: imageB64,
                model: model,
                sourceLang: srcLang,
                targetLang: tgtLang,
                vision: vision,
                skipCache: skipCache
            )

        case "saveExportedImage":
            let dataUrl = (payload["data_url"] as? String) ?? ""
            let suggestedName = (payload["suggested_name"] as? String) ?? "manga-translated.png"
            handleSaveExportedImage(callId: callId, dataUrl: dataUrl, suggestedName: suggestedName)

        default:
            resolveBridgeCall(id: callId, ok: false, payload: ["error": "Action inconnue: \(action)"])
        }
    }

    nonisolated private func persistConfigUpdates(_ payload: [String: Any]) -> [String: Any] {
        let dataDir = appSupportDataDir()
        try? FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
        var current = loadAppConfig(dataDir: dataDir)
        let oldProvider = ((current["api_provider"] as? String) ?? "gemini").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        for key in ["onboarding_done", "ui_lang", "theme", "mascot", "engine_mode", "default_model", "api_provider", "api_base_url", "api_model", "source_lang", "target_lang", "vision_mode", "userscripts_dir"] {
            if let val = payload[key] { current[key] = val }
        }
        let newProvider = ((current["api_provider"] as? String) ?? "gemini").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        current["api_provider"] = kProviderDefaults[newProvider] != nil ? newProvider : "gemini"
        let resolvedProvider = current["api_provider"] as? String ?? "gemini"
        let trimmedBase = ((current["api_base_url"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        current["api_base_url"] = trimmedBase.hasSuffix("/") ? String(trimmedBase.dropLast()) : trimmedBase

        let newApiKey = (payload["api_key"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let newGemKey = (payload["gemini_api_key"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let effectiveNewKey = !newApiKey.isEmpty ? newApiKey : newGemKey

        if let clearKey = payload["clear_api_key"] as? Bool, clearKey {
            current["api_key"] = ""
            current["gemini_api_key"] = ""
        } else if !effectiveNewKey.isEmpty {
            current["api_key"] = effectiveNewKey
            if resolvedProvider == "gemini" {
                current["gemini_api_key"] = effectiveNewKey
            }
        } else if resolvedProvider != oldProvider {
            current["api_key"] = resolvedProvider == "gemini" ? ((current["gemini_api_key"] as? String) ?? "") : ""
        }
        if let t = current["theme"] as? String {
            Task { @MainActor [weak self] in self?.applyWindowTheme(t) }
        }
        let configURL = dataDir.appendingPathComponent("config.json")
        if let jsonData = try? JSONSerialization.data(withJSONObject: current, options: [.prettyPrinted]) {
            try? jsonData.write(to: configURL)
            try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: configURL.path)
        }
        return current
    }

    nonisolated private func handleInstallPython(callId: String) {
        let dataDir = appSupportDataDir()
        let runtimeDir = dataDir.appendingPathComponent("runtime", isDirectory: true)
        let fm = FileManager.default
        try? fm.createDirectory(at: runtimeDir, withIntermediateDirectories: true)
        let archiveURL = dataDir.appendingPathComponent("python-arm64.tar.gz")

        guard let dlURL = URL(string: kPythonStandaloneURL) else {
            resolveBridgeCall(id: callId, ok: false, payload: ["error": "URL Python invalide"])
            return
        }

        let helper = DownloadHelper(
            onProgress: { [weak self] done, total in
                let pct = total > 0 ? Int((Double(done) / Double(total)) * 100.0) : -1
                self?.pushEvent(name: "progress", payload: [
                    "tool": "python", "phase": "downloading", "completed": done, "total": total, "percent": pct
                ])
            },
            onComplete: { [weak self] result in
                guard let self = self else { return }
                self.runtime.withLock { $0.activeDownload = nil }
                switch result {
                case .failure(let err):
                    self.resolveBridgeCall(id: callId, ok: false, payload: ["error": err.localizedDescription])
                case .success(let tarFile):
                    self.pushEvent(name: "progress", payload: ["tool": "python", "phase": "extracting", "percent": 100])
                    let pythonRoot = runtimeDir.appendingPathComponent("python", isDirectory: true)
                    if fm.fileExists(atPath: pythonRoot.path) { try? fm.removeItem(at: pythonRoot) }
                    let extRes = runSyncProcess("/usr/bin/tar", ["-xzf", tarFile.path, "-C", runtimeDir.path])
                    try? fm.removeItem(at: tarFile)
                    if extRes.exitCode != 0 {
                        self.resolveBridgeCall(id: callId, ok: false, payload: ["error": extRes.stderr])
                        return
                    }
                    let pyInfo = detectPython(dataDir: dataDir)
                    let ok = (pyInfo["installed"] as? Bool) ?? false
                    self.resolveBridgeCall(id: callId, ok: ok, payload: inspectSystemState())
                }
            }
        )
        runtime.withLock { $0.activeDownload = helper }
        helper.start(url: dlURL, dest: archiveURL)
    }

    nonisolated private func handleInstallOllama(callId: String) {
        let dataDir = appSupportDataDir()
        let fm = FileManager.default
        try? fm.createDirectory(at: dataDir, withIntermediateDirectories: true)
        let zipURL = dataDir.appendingPathComponent("Ollama-darwin.zip")

        guard let dlURL = URL(string: kOllamaZipURL) else {
            resolveBridgeCall(id: callId, ok: false, payload: ["error": "URL Ollama invalide"])
            return
        }

        let helper = DownloadHelper(
            onProgress: { [weak self] done, total in
                let pct = total > 0 ? Int((Double(done) / Double(total)) * 100.0) : -1
                self?.pushEvent(name: "progress", payload: [
                    "tool": "ollama", "phase": "downloading", "completed": done, "total": total, "percent": pct
                ])
            },
            onComplete: { [weak self] result in
                guard let self = self else { return }
                self.runtime.withLock { $0.activeDownload = nil }
                switch result {
                case .failure(let err):
                    self.resolveBridgeCall(id: callId, ok: false, payload: ["error": err.localizedDescription])
                case .success(let file):
                    self.pushEvent(name: "progress", payload: ["tool": "ollama", "phase": "extracting", "percent": 100])
                    let userApps = fm.homeDirectoryForCurrentUser.appendingPathComponent("Applications", isDirectory: true)
                    try? fm.createDirectory(at: userApps, withIntermediateDirectories: true)
                    var targetAppsDir = "/Applications"
                    if !fm.isWritableFile(atPath: "/Applications") {
                        targetAppsDir = userApps.path
                    }
                    let existing = "\(targetAppsDir)/Ollama.app"
                    if fm.fileExists(atPath: existing) {
                        if (try? fm.removeItem(atPath: existing)) == nil {
                            targetAppsDir = userApps.path
                            let userExisting = "\(targetAppsDir)/Ollama.app"
                            if fm.fileExists(atPath: userExisting) { try? fm.removeItem(atPath: userExisting) }
                        }
                    }
                    var extRes = runSyncProcess("/usr/bin/ditto", ["-xk", file.path, targetAppsDir])
                    if extRes.exitCode != 0 && targetAppsDir == "/Applications" {
                        targetAppsDir = userApps.path
                        extRes = runSyncProcess("/usr/bin/ditto", ["-xk", file.path, targetAppsDir])
                    }
                    try? fm.removeItem(at: file)
                    if extRes.exitCode != 0 {
                        self.resolveBridgeCall(id: callId, ok: false, payload: ["error": extRes.stderr])
                        return
                    }
                    let started = self.ensureOllamaRunning()
                    self.resolveBridgeCall(id: callId, ok: started || detectOllama()["installed"] as? Bool == true, payload: inspectSystemState())
                }
            }
        )
        runtime.withLock { $0.activeDownload = helper }
        helper.start(url: dlURL, dest: zipURL)
    }

    nonisolated private func ensureOllamaRunning() -> Bool {
        let (st, _, _) = httpJSONSync(urlString: "\(kOllamaBaseURL)/api/tags", timeout: 1.0)
        if st == 200 { return true }

        let (binPath, appPath) = findOllamaBinary()
        let ollamaEnv = [
            "OLLAMA_FLASH_ATTENTION": "1",
            "OLLAMA_KV_CACHE_TYPE": "q8_0",
        ]
        if !binPath.isEmpty {
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: binPath)
            proc.arguments = ["serve"]
            var merged = ProcessInfo.processInfo.environment
            for (k, v) in ollamaEnv { merged[k] = v }
            proc.environment = merged
            let logURL = appSupportDataDir().appendingPathComponent("ollama.log")
            FileManager.default.createFile(atPath: logURL.path, contents: nil)
            if let fh = try? FileHandle(forWritingTo: logURL) {
                proc.standardOutput = fh
                proc.standardError = fh
            }
            if (try? proc.run()) != nil {
                runtime.withLock { $0.ollamaProcess = proc }
            }
        } else if let app = appPath {
            _ = runSyncProcess("/usr/bin/open", ["-g", "-a", app], env: ollamaEnv)
        }

        for _ in 0..<24 {
            Thread.sleep(forTimeInterval: 0.5)
            let (poll, _, _) = httpJSONSync(urlString: "\(kOllamaBaseURL)/api/tags", timeout: 1.0)
            if poll == 200 { return true }
        }
        return false
    }

    nonisolated private func handlePullModel(callId: String, modelName: String) {
        if !ensureOllamaRunning() {
            resolveBridgeCall(id: callId, ok: false, payload: ["error": "Ollama ne répond pas sur 127.0.0.1:11434"])
            return
        }
        let pull = PullStreamHelper(
            onLine: { [weak self] line in
                let status = (line["status"] as? String) ?? "Téléchargement..."
                let done = (line["completed"] as? NSNumber)?.int64Value ?? 0
                let total = (line["total"] as? NSNumber)?.int64Value ?? 0
                let pct = total > 0 ? min(100, Int((Double(done) / Double(total)) * 100.0)) : -1
                if let errMsg = line["error"] as? String {
                    self?.pushEvent(name: "progress", payload: [
                        "tool": "pull", "model": modelName, "status": "Erreur: \(errMsg)", "percent": 0
                    ])
                    return
                }
                self?.pushEvent(name: "progress", payload: [
                    "tool": "pull", "model": modelName, "status": status, "completed": done, "total": total, "percent": pct
                ])
            },
            onFinish: { [weak self] result in
                guard let self = self else { return }
                self.runtime.withLock { $0.activePull = nil }
                switch result {
                case .failure(let err):
                    self.resolveBridgeCall(id: callId, ok: false, payload: ["error": err.localizedDescription])
                case .success:
                    self.resolveBridgeCall(id: callId, ok: true, payload: inspectSystemState())
                }
            }
        )
        runtime.withLock { $0.activePull = pull }
        pull.start(model: modelName)
    }

    nonisolated private func installUserscriptFile(to folder: URL) throws -> URL {
        guard let srcURL = resolveResourceURL("userscript/yomi.user.js") else {
            throw NSError(domain: "Yomi", code: 404, userInfo: [NSLocalizedDescriptionKey: "Userscript introuvable"])
        }
        let fm = FileManager.default
        let legacyDst = folder.appendingPathComponent("safari-manga-translator.user.js")
        if fm.fileExists(atPath: legacyDst.path) {
            try? fm.removeItem(at: legacyDst)
        }
        let dst = folder.appendingPathComponent("yomi.user.js")
        if fm.fileExists(atPath: dst.path) {
            let srcData = try? Data(contentsOf: srcURL)
            let dstData = try? Data(contentsOf: dst)
            if let s = srcData, let d = dstData, s == d {
                return dst
            }
            try fm.removeItem(at: dst)
        }
        try fm.copyItem(at: srcURL, to: dst)
        return dst
    }

    nonisolated private func syncUserscriptIfConfigured() {
        let cfg = loadAppConfig(dataDir: appSupportDataDir())
        let info = detectUserscriptsInfo(cfg: cfg)
        let dir = (info["detected_dir"] as? String) ?? ""
        let savedDir = ((cfg["userscripts_dir"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let alreadyInstalled = (info["script_installed"] as? Bool) ?? false
        let legacyExists = !dir.isEmpty && FileManager.default.fileExists(atPath: "\(dir)/safari-manga-translator.user.js")
        guard !dir.isEmpty && (alreadyInstalled || legacyExists || !savedDir.isEmpty) else { return }
        _ = try? installUserscriptFile(to: URL(fileURLWithPath: dir, isDirectory: true))
    }

    private func handleInstallUserscriptPanel(callId: String) {
        guard resolveResourceURL("userscript/yomi.user.js") != nil else {
            resolveBridgeCall(id: callId, ok: false, payload: ["error": "Userscript introuvable"])
            return
        }
        let cfg = loadAppConfig(dataDir: appSupportDataDir())
        let info = detectUserscriptsInfo(cfg: cfg)
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        if let dir = info["detected_dir"] as? String, !dir.isEmpty {
            panel.directoryURL = URL(fileURLWithPath: dir, isDirectory: true)
        }
        panel.message = "Choisis le dossier de ton extension (ex: le dossier Userscripts de Safari)"
        panel.prompt = "Installer ici"
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let folder = panel.url else {
                self.resolveBridgeCall(id: callId, ok: true, payload: ["cancelled": true])
                return
            }
            do {
                let dst = try self.installUserscriptFile(to: folder)
                _ = self.persistConfigUpdates(["userscripts_dir": folder.path])
                self.resolveBridgeCall(id: callId, ok: true, payload: ["path": dst.path, "state": inspectSystemState()])
            } catch {
                self.resolveBridgeCall(id: callId, ok: false, payload: ["error": error.localizedDescription])
            }
        }
    }

    nonisolated private func readImageFileAsDataURL(url: URL) -> Result<(dataUrl: String, filename: String), Error> {
        do {
            let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
            if let size = attrs[.size] as? NSNumber, size.int64Value > 30 * 1024 * 1024 {
                throw NSError(domain: "Yomi", code: 413, userInfo: [NSLocalizedDescriptionKey: "Image trop volumineuse (max 30 Mo)"])
            }
            let ext = url.pathExtension.lowercased()
            let directMimes: [String: String] = [
                "png": "image/png",
                "jpg": "image/jpeg",
                "jpeg": "image/jpeg",
                "webp": "image/webp",
                "gif": "image/gif",
            ]
            if let mime = directMimes[ext] {
                let raw = try Data(contentsOf: url)
                return .success(("data:\(mime);base64,\(raw.base64EncodedString())", url.lastPathComponent))
            }
            guard let img = NSImage(contentsOf: url),
                  let tiff = img.tiffRepresentation,
                  let rep = NSBitmapImageRep(data: tiff),
                  let pngData = rep.representation(using: .png, properties: [:]) else {
                throw NSError(domain: "Yomi", code: 415, userInfo: [NSLocalizedDescriptionKey: "Format d'image non pris en charge"])
            }
            return .success(("data:image/png;base64,\(pngData.base64EncodedString())", url.lastPathComponent))
        } catch {
            return .failure(error)
        }
    }

    private func handlePickTestImage(callId: String) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.allowedContentTypes = [.image]
        panel.title = "Choisir une planche de manga"
        panel.prompt = "Ouvrir"
        panel.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            guard response == .OK, let url = panel.url else {
                self.resolveBridgeCall(id: callId, ok: true, payload: ["cancelled": true])
                return
            }
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                guard let self else { return }
                switch self.readImageFileAsDataURL(url: url) {
                case .success(let res):
                    self.resolveBridgeCall(id: callId, ok: true, payload: [
                        "data_url": res.dataUrl,
                        "filename": res.filename,
                    ])
                case .failure(let err):
                    self.resolveBridgeCall(id: callId, ok: false, payload: ["error": err.localizedDescription])
                }
            }
        }
    }

    private func readClipboardImageDataURL() -> (dataUrl: String, filename: String)? {
        let pb = NSPasteboard.general
        if let pngData = pb.data(forType: .png), !pngData.isEmpty {
            return ("data:image/png;base64,\(pngData.base64EncodedString())", "clipboard.png")
        }
        if let img = NSImage(pasteboard: pb),
           let tiff = img.tiffRepresentation,
           let rep = NSBitmapImageRep(data: tiff),
           let pngData = rep.representation(using: .png, properties: [:]) {
            return ("data:image/png;base64,\(pngData.base64EncodedString())", "clipboard.png")
        }
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] {
            for url in urls {
                if let ut = UTType(filenameExtension: url.pathExtension), ut.conforms(to: .image) {
                    if case .success(let res) = readImageFileAsDataURL(url: url) {
                        return res
                    }
                }
            }
        }
        return nil
    }

    nonisolated private func handleGetTestHealth(callId: String, autoStart: Bool) {
        let (st, json, errTxt) = httpJSONSync(urlString: "\(kServerBaseURL)/api/health", timeout: 2.0)
        if st == 200, let json {
            resolveBridgeCall(id: callId, ok: true, payload: ["health": json, "state": inspectSystemState()])
            return
        }
        if !autoStart {
            resolveBridgeCall(id: callId, ok: false, payload: [
                "error": errTxt.isEmpty ? "Serveur local indisponible" : errTxt,
                "state": inspectSystemState(),
            ])
            return
        }
        startBackendIfPossible(forceRestart: false) { [weak self] ok in
            guard let self else { return }
            if ok {
                let (st2, json2, err2) = httpJSONSync(urlString: "\(kServerBaseURL)/api/health", timeout: 4.0)
                if st2 == 200, let json2 {
                    self.resolveBridgeCall(id: callId, ok: true, payload: ["health": json2, "state": inspectSystemState()])
                    return
                }
                self.resolveBridgeCall(id: callId, ok: false, payload: [
                    "error": err2.isEmpty ? "Serveur local indisponible" : err2,
                    "state": inspectSystemState(),
                ])
            } else {
                self.resolveBridgeCall(id: callId, ok: false, payload: [
                    "error": "Serveur local indisponible (vérifiez Python dans Réglages).",
                    "state": inspectSystemState(),
                ])
            }
        }
    }

    nonisolated private func handleTranslateTestImage(
        callId: String,
        imageBase64: String,
        model: String,
        sourceLang: String,
        targetLang: String,
        vision: String,
        skipCache: Bool
    ) {
        if imageBase64.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            resolveBridgeCall(id: callId, ok: false, payload: ["error": "Image manquante"])
            return
        }

        let performRequest: @Sendable () -> Void = { [weak self] in
            guard let self else { return }
            var body: [String: Any] = [
                "image_base64": imageBase64,
                "source_lang": sourceLang,
                "target_lang": targetLang,
                "vision": vision,
                "skip_cache": skipCache,
            ]
            if !model.isEmpty {
                body["model"] = model
            }
            let (status, json, errTxt) = httpJSONSync(
                urlString: "\(kServerBaseURL)/api/translate",
                method: "POST",
                body: body,
                timeout: 180.0
            )
            if status == 200, let json {
                self.resolveBridgeCall(id: callId, ok: true, payload: json)
            } else {
                let msg = (json?["error"] as? String) ?? (errTxt.isEmpty ? "HTTP \(status)" : errTxt)
                self.resolveBridgeCall(id: callId, ok: false, payload: ["error": msg])
            }
        }

        let (healthStatus, _, _) = httpJSONSync(urlString: "\(kServerBaseURL)/api/health", timeout: 1.0)
        if healthStatus == 200 {
            performRequest()
            return
        }

        startBackendIfPossible(forceRestart: false) { [weak self] ok in
            guard let self else { return }
            if ok {
                performRequest()
            } else {
                self.resolveBridgeCall(id: callId, ok: false, payload: [
                    "error": "Serveur local indisponible (vérifiez Python dans Réglages)."
                ])
            }
        }
    }

    nonisolated private func handleSaveExportedImage(callId: String, dataUrl: String, suggestedName: String) {
        var b64 = dataUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        if let range = b64.range(of: ";base64,") {
            b64 = String(b64[range.upperBound...])
        }
        guard let pngData = Data(base64Encoded: b64, options: .ignoreUnknownCharacters), !pngData.isEmpty else {
            resolveBridgeCall(id: callId, ok: false, payload: ["error": "Image PNG invalide"])
            return
        }
        let cleanName = suggestedName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? "manga-translated.png"
            : suggestedName
        Task { @MainActor [weak self] in
            guard let self else { return }
            let panel = NSSavePanel()
            panel.allowedContentTypes = [.png]
            panel.nameFieldStringValue = cleanName
            panel.canCreateDirectories = true
            panel.isExtensionHidden = false
            panel.title = "Exporter la planche traduite"
            panel.prompt = "Exporter"
            panel.beginSheetModal(for: self.window) { response in
                guard response == .OK, let url = panel.url else {
                    self.resolveBridgeCall(id: callId, ok: true, payload: ["cancelled": true])
                    return
                }
                do {
                    try pngData.write(to: url)
                    self.resolveBridgeCall(id: callId, ok: true, payload: ["saved": true, "path": url.path])
                } catch {
                    self.resolveBridgeCall(id: callId, ok: false, payload: ["error": error.localizedDescription])
                }
            }
        }
    }

    nonisolated private func stopExistingServer() {
        let dataDir = appSupportDataDir()
        let pidFile = dataDir.appendingPathComponent("server.pid")
        let (st, _, _) = httpJSONSync(urlString: "\(kServerBaseURL)/api/unload", method: "POST", body: [:], timeout: 1.5)
        _ = st
        let srv = runtime.withLock { box -> Process? in
            let p = box.serverProcess
            box.serverProcess = nil
            return p
        }
        if let srv = srv, srv.isRunning {
            srv.terminate()
            srv.waitUntilExit()
        }
        if let pidTxt = try? String(contentsOf: pidFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines),
           let pid = Int32(pidTxt), pid > 1 {
            kill(pid, SIGTERM)
        }
        try? FileManager.default.removeItem(at: pidFile)

        // Arrête uniquement un éventuel ancien server.py de Yomi sur le port 8765
        let res = runSyncProcess("/usr/bin/pgrep", ["-f", "(yomi|safari-manga-translator).*server\\.py|Resources/server\\.py"])
        if res.exitCode == 0 && !res.stdout.isEmpty {
            for line in res.stdout.split(separator: "\n") {
                if let p = Int32(line.trimmingCharacters(in: .whitespaces)), p > 1 && p != getpid() {
                    kill(p, SIGTERM)
                }
            }
            Thread.sleep(forTimeInterval: 0.4)
        }
    }

    nonisolated private func startBackendIfPossible(forceRestart: Bool = false, completion: @escaping @Sendable (Bool) -> Void) {
        let dataDir = appSupportDataDir()
        if !forceRestart {
            let (st, _, _) = httpJSONSync(urlString: "\(kServerBaseURL)/api/health", timeout: 1.0)
            if st == 200 {
                Task { @MainActor [weak self] in self?.refreshMenuStatus(true) }
                completion(true)
                return
            }
        }
        if forceRestart {
            stopExistingServer()
        }
        let py = detectPython(dataDir: dataDir)
        guard (py["installed"] as? Bool) == true,
              let pyPath = py["path"] as? String,
              let serverURL = resolveResourceURL("server.py") else {
            Task { @MainActor [weak self] in self?.refreshMenuStatus(false) }
            completion(false)
            return
        }

        let cfg = loadAppConfig(dataDir: dataDir)
        if (cfg["engine_mode"] as? String ?? "local") != "api" {
            let (_, _, _) = httpJSONSync(urlString: "\(kOllamaBaseURL)/api/tags", timeout: 0.8)
            _ = ensureOllamaRunning()
        }

        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: pyPath)
        proc.arguments = ["-u", serverURL.path]
        var env = ProcessInfo.processInfo.environment
        env["PYTHONUNBUFFERED"] = "1"
        env["SMT_DATA_DIR"] = dataDir.path
        if let ocrDir = resolveResourceURL("ocr") {
            env["SMT_OCR_DIR"] = ocrDir.path
        }
        if let usURL = resolveResourceURL("userscript/yomi.user.js") {
            env["SMT_USERSCRIPT_PATH"] = usURL.path
        }
        env["OLLAMA_FLASH_ATTENTION"] = "1"
        env["OLLAMA_KV_CACHE_TYPE"] = "q8_0"
        proc.environment = env

        let logURL = dataDir.appendingPathComponent("server.log")
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        if let fh = try? FileHandle(forWritingTo: logURL) {
            proc.standardOutput = fh
            proc.standardError = fh
        }

        do {
            try proc.run()
            runtime.withLock { $0.serverProcess = proc }
            try? "\(proc.processIdentifier)".write(to: dataDir.appendingPathComponent("server.pid"), atomically: true, encoding: .utf8)
        } catch {
            completion(false)
            return
        }

        var ready = false
        for _ in 0..<24 {
            Thread.sleep(forTimeInterval: 0.5)
            let (st, _, _) = httpJSONSync(urlString: "\(kServerBaseURL)/api/health", timeout: 1.0)
            if st == 200 {
                ready = true
                break
            }
        }
        let isReady = ready
        Task { @MainActor [weak self] in self?.refreshMenuStatus(isReady) }
        completion(ready)
    }

    nonisolated private func performUnloadRAM() -> [String] {
        let (_, json, _) = httpJSONSync(urlString: "\(kServerBaseURL)/api/unload", method: "POST", body: [:], timeout: 3.0)
        return (json?["unloaded"] as? [String]) ?? []
    }

    nonisolated private func performClearCache() -> Int {
        let (st, json, _) = httpJSONSync(urlString: "\(kServerBaseURL)/api/clear-cache", method: "POST", body: [:], timeout: 3.0)
        if st == 200 {
            return (json?["deleted"] as? Int) ?? 0
        }
        let cacheDir = appSupportDataDir().appendingPathComponent("cache", isDirectory: true)
        var count = 0
        if let items = try? FileManager.default.contentsOfDirectory(at: cacheDir, includingPropertiesForKeys: nil) {
            for item in items where item.pathExtension == "json" {
                if (try? FileManager.default.removeItem(at: item)) != nil { count += 1 }
            }
        }
        return count
    }

    nonisolated private func shutdownBackgroundProcesses() {
        _ = performUnloadRAM()
        stopExistingServer()
        let oll = runtime.withLock { box -> Process? in
            let p = box.ollamaProcess
            box.ollamaProcess = nil
            return p
        }
        if let oll = oll, oll.isRunning {
            oll.terminate()
        }
    }
}

if CommandLine.arguments.contains("--self-test") {
    let report = inspectSystemState()
    if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]),
       let str = String(data: data, encoding: .utf8) {
        print(str)
    }
    exit(0)
}

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.run()
}
