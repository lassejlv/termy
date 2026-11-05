import SwiftUI
import TermyRemoteKit

struct TerminalScreenView: View {
    let link: HostLink
    let screen: TerminalScreen
    @Environment(\.dismiss) private var dismiss
    @State private var terminalView: TerminalUIView?

    var body: some View {
        NavigationStack {
            TerminalViewRepresentable(screen: screen, link: link) { view in
                DispatchQueue.main.async { terminalView = view }
            }
            .ignoresSafeArea(.container, edges: .bottom)
            .background(Color(red: 0.06, green: 0.08, blue: 0.13))
            .overlay(alignment: .top) { banner }
            .navigationTitle(screen.title ?? "Session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Done") {
                        link.close(screen)
                        dismiss()
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(screen.fitted ? "Restore Size" : "Fit to iPhone",
                           systemImage: screen.fitted ? "arrow.up.left.and.arrow.down.right" : "iphone") {
                        toggleFit()
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var banner: some View {
        if screen.exited {
            Label("The program ended.", systemImage: "stop.circle")
                .bannerStyle()
        } else if !screen.live {
            Label(link.state == .offline ? "\(link.device.name) is offline" : "Connecting…", systemImage: "wifi.exclamationmark")
                .bannerStyle()
        }
    }

    private func toggleFit() {
        if screen.fitted {
            link.send(.resize(stream: screen.stream, cols: nil, rows: nil))
            screen.fitted = false
        } else if let grid = terminalView?.readableGrid {
            link.send(.resize(stream: screen.stream, cols: grid.cols, rows: grid.rows))
            screen.fitted = true
        }
    }
}

private extension View {
    func bannerStyle() -> some View {
        font(.footnote.weight(.medium))
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(.ultraThinMaterial, in: Capsule())
            .padding(.top, 8)
    }
}
