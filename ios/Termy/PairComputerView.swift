@preconcurrency import AVFoundation
import SwiftUI
import TermyRemoteKit

struct PairComputerView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var code: PairingCode?
    @State private var error: String?
    @State private var cameraAllowed = false
    @State private var pairingTask: Task<Void, Never>?
    var body: some View {
        NavigationStack {
            VStack(spacing: 20) {
                if let code {
                    Image(systemName: "macbook.and.iphone").font(.system(size: 50))
                    Text("Connect to \(code.hostName)?").font(.title2.bold())
                    Text("Relay: \(code.relayURL.host ?? code.relayURL.absoluteString)").font(.callout).foregroundStyle(
                        .secondary)
                    Text(
                        "After connecting, approve this iPhone on your Mac. It will be able to view and control that Mac’s Termy Next sessions."
                    )
                    .multilineTextAlignment(.center)
                    if model.pairingBusy {
                        ProgressView()
                        Text(model.pairingMessage ?? "Pairing…").multilineTextAlignment(.center)
                    } else {
                        Button("Connect") {
                            error = nil
                            pairingTask = Task {
                                do {
                                    try await model.pair(code)
                                    dismiss()
                                } catch is CancellationError {} catch { self.error = error.localizedDescription }
                            }
                        }.buttonStyle(.borderedProminent)
                        Button("Scan Another Code") {
                            self.code = nil
                            error = nil
                        }
                    }
                } else {
                    Text("Scan the QR code in Termy Next → Settings → Mobile.")
                        .multilineTextAlignment(.center).foregroundStyle(.secondary)
                    if cameraAllowed {
                        QRScanner(onCode: scanned, onError: { error = $0 })
                            .frame(maxHeight: 340).clipShape(RoundedRectangle(cornerRadius: 20))
                    } else {
                        ContentUnavailableView(
                            "Camera Access Needed", systemImage: "camera",
                            description: Text("Allow camera access in iPhone Settings to scan your Mac’s pairing code.")
                        )
                    }
                    Button("Paste Pairing Code", systemImage: "doc.on.clipboard") {
                        if let text = UIPasteboard.general.string { scanned(text) }
                    }
                }
                if let error { Text(error).foregroundStyle(.red).multilineTextAlignment(.center) }
                Spacer(minLength: 0)
            }
            .padding(24)
            .navigationTitle("Pair Computer").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        pairingTask?.cancel()
                        dismiss()
                    }
                }
            }
            .task { cameraAllowed = await AVCaptureDevice.requestAccess(for: .video) }
            .onDisappear { pairingTask?.cancel() }
        }
    }
    private func scanned(_ text: String) {
        guard code == nil else { return }
        do {
            code = try PairingCode.parse(text)
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

private struct QRScanner: UIViewControllerRepresentable {
    var onCode: (String) -> Void
    var onError: (String) -> Void
    func makeUIViewController(context: Context) -> QRScannerController {
        QRScannerController(onCode: onCode, onError: onError)
    }
    func updateUIViewController(_ controller: QRScannerController, context: Context) {}
}
private final class QRScannerController: UIViewController, @preconcurrency AVCaptureMetadataOutputObjectsDelegate {
    private let capture = AVCaptureSession()
    private let captureQueue = DispatchQueue(label: "sh.termy.qr-camera")
    private var preview: AVCaptureVideoPreviewLayer?
    private let onCode: (String) -> Void
    private let onError: (String) -> Void
    init(onCode: @escaping (String) -> Void, onError: @escaping (String) -> Void) {
        self.onCode = onCode
        self.onError = onError
        super.init(nibName: nil, bundle: nil)
    }
    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }
    override func viewDidLoad() {
        super.viewDidLoad()
        guard let camera = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: camera),
            capture.canAddInput(input)
        else {
            onError("No camera is available. You can paste a pairing code instead.")
            return
        }
        capture.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard capture.canAddOutput(output) else {
            onError("The camera cannot scan QR codes.")
            return
        }
        capture.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        let preview = AVCaptureVideoPreviewLayer(session: capture)
        preview.videoGravity = .resizeAspectFill
        self.preview = preview
        view.layer.addSublayer(preview)
    }
    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        let capture = capture
        captureQueue.async { capture.startRunning() }
    }
    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        let capture = capture
        captureQueue.async { capture.stopRunning() }
    }
    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
    }
    func metadataOutput(
        _ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection
    ) {
        guard let text = objects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first else {
            return
        }
        onCode(text)
    }
}
