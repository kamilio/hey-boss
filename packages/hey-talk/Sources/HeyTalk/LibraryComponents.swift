import SwiftUI

/// Shared by Hey Talk's history and vocabulary windows.
struct LibraryHeader: View {
    let subtitle: String
    let badge: String
    let symbol: String

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: "waveform")
                .font(.system(size: 24, weight: .medium)).foregroundStyle(.teal)
                .frame(width: 52, height: 52)
                .background(.teal.opacity(0.10), in: RoundedRectangle(cornerRadius: 16))
            VStack(alignment: .leading, spacing: 4) {
                Text("Hey Talk").font(.system(size: 26, weight: .semibold, design: .rounded))
                Text(subtitle).font(.system(size: 13)).foregroundStyle(.secondary)
            }
            Spacer()
            Label(badge, systemImage: symbol)
                .font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 26).padding(.top, 38).padding(.bottom, 24)
    }
}

struct LibraryToolbar: View {
    let title: String
    let placeholder: String
    @Binding var search: String

    var body: some View {
        HStack {
            Text(title).font(.system(size: 14, weight: .semibold))
            Spacer()
            HStack(spacing: 7) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField(placeholder, text: $search).textFieldStyle(.plain)
                    .accessibilityLabel(placeholder)
                if !search.isEmpty {
                    Button { search = "" } label: { Image(systemName: "xmark.circle.fill") }
                        .buttonStyle(.plain).accessibilityLabel("Clear search")
                }
            }
            .font(.system(size: 12)).padding(9).frame(width: 220)
            .background(.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 10))
        }.padding(.horizontal, 26).padding(.bottom, 14)
        Divider().padding(.horizontal, 26)
    }
}

struct LibraryRows<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        ScrollView {
            LazyVStack(spacing: 0) { content }.padding(8)
        }
        .padding(.horizontal, 10)
    }
}

struct LibraryRow<Content: View>: View {
    var selected = false
    @ViewBuilder var content: Content
    @State private var hovered = false

    var body: some View {
        content.padding(18)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.teal.opacity(selected ? 0.10 : hovered ? 0.055 : 0), in: RoundedRectangle(cornerRadius: 14))
            .onHover { hovered = $0 }
    }
}

struct LibraryEmptyState: View {
    let symbol: String
    let title: String
    let detail: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: symbol).font(.system(size: 34, weight: .light)).foregroundStyle(.teal)
            Text(title).font(.system(size: 17, weight: .medium))
            Text(detail).font(.callout).foregroundStyle(.secondary)
        }
        .multilineTextAlignment(.center)
    }
}
