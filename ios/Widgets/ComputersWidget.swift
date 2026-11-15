import SwiftUI
import WidgetKit

@main
struct TermyWidgets: WidgetBundle {
    var body: some Widget { ComputersWidget() }
}

struct ComputersEntry: TimelineEntry {
    let date: Date
    let snapshot: HostSnapshot
}

struct ComputersProvider: TimelineProvider {
    static let sample = HostSnapshot(
        hosts: [
            .init(id: UUID(), name: "MacBook Pro", online: true, panes: 4, running: 1),
            .init(id: UUID(), name: "Studio", online: false, panes: nil),
        ], updatedAt: Date())

    func placeholder(in context: Context) -> ComputersEntry { ComputersEntry(date: Date(), snapshot: Self.sample) }

    func getSnapshot(in context: Context, completion: @escaping (ComputersEntry) -> Void) {
        let snapshot = HostSnapshotStore.load()
        completion(ComputersEntry(date: Date(), snapshot: context.isPreview && snapshot.hosts.isEmpty ? Self.sample : snapshot))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<ComputersEntry>) -> Void) {
        // The app reloads the widget whenever what it saw changes.
        completion(Timeline(entries: [ComputersEntry(date: Date(), snapshot: HostSnapshotStore.load())], policy: .never))
    }
}

struct ComputersWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "sh.termy.computers", provider: ComputersProvider()) { entry in
            ComputersWidgetView(entry: entry).containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Computers")
        .description("Your paired Macs and whether Termy can reach them.")
        .supportedFamilies([.systemSmall, .systemMedium, .accessoryRectangular, .accessoryInline])
    }
}

struct ComputersWidgetView: View {
    let entry: ComputersEntry
    @Environment(\.widgetFamily) private var family

    private var hosts: [HostSnapshot.Host] { entry.snapshot.hosts }

    var body: some View {
        if hosts.isEmpty {
            empty
        } else {
            switch family {
            case .accessoryInline:
                Label("\(entry.snapshot.onlineCount) of \(hosts.count) Macs online", systemImage: "desktopcomputer")
            case .accessoryRectangular:
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(hosts.prefix(3)) { host in
                        Label(host.name, systemImage: host.online ? "circle.fill" : "circle").font(.caption)
                    }
                }
            case .systemSmall: small
            default: medium
            }
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 6) {
            Image(systemName: "macbook.and.iphone").font(.title2)
            Text("Pair a Mac").font(.headline)
            Text("Open Termy to scan the code from Termy Next.").font(.caption).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .widgetURL(TermyLink.pair)
    }

    private var small: some View {
        let host = hosts.first(where: \.online) ?? hosts[0]
        return VStack(alignment: .leading, spacing: 4) {
            HStack {
                Image(systemName: "terminal.fill").foregroundStyle(.tint)
                Spacer()
                Text("\(entry.snapshot.onlineCount)/\(hosts.count)").font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            Text(host.name).font(.headline).lineLimit(2)
            status(host)
            updated
        }
        .widgetURL(TermyLink.open(host: host.id))
    }

    private var medium: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Label("Computers", systemImage: "terminal.fill").font(.caption.weight(.semibold)).foregroundStyle(.tint)
                Spacer()
                updated
            }
            ForEach(hosts.prefix(3)) { host in
                Link(destination: TermyLink.open(host: host.id)) {
                    HStack {
                        Text(host.name).font(.subheadline.weight(.medium)).lineLimit(1)
                        Spacer()
                        status(host)
                    }
                }
            }
            Spacer(minLength: 0)
        }
    }

    private func status(_ host: HostSnapshot.Host) -> some View {
        HStack(spacing: 4) {
            Circle().fill(host.online ? Color.green : Color.secondary).frame(width: 6, height: 6)
            if host.online, let running = host.running, running > 0 {
                Text("\(running) running")
            } else if host.online, let panes = host.panes {
                Text("\(panes) pane\(panes == 1 ? "" : "s")")
            } else {
                Text(host.online ? "Online" : "Offline")
            }
        }
        .font(.caption).foregroundStyle(.secondary)
    }

    @ViewBuilder private var updated: some View {
        if entry.snapshot.updatedAt > .distantPast {
            Text(entry.snapshot.updatedAt, style: .relative).font(.caption2).foregroundStyle(.tertiary)
        }
    }
}
