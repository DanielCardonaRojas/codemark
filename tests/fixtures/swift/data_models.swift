import Foundation

// MARK: - Nested types (tests deep structural path generation)

struct User {
    let id: UUID
    let name: String
    let email: String
    let profile: Profile

    struct Profile {
        let bio: String
        let avatarURL: URL?
        let settings: Settings

        struct Settings {
            let theme: Theme
            let notifications: Bool
            let language: String

            // Target: bookmark a deeply nested enum — tests 4-level structural path.
            enum Theme: String, Codable {
                case light
                case dark
                case system
            }
        }
    }
}

// MARK: - Protocol with default implementations

protocol Identifiable {
    associatedtype ID: Hashable
    var id: ID { get }
}

extension Identifiable where ID == UUID {
    // Target: bookmark a default implementation in a constrained extension.
    func matches(_ other: Self) -> Bool {
        return self.id == other.id
    }
}

// MARK: - Computed properties and closures

struct Pagination {
    let page: Int
    let perPage: Int
    let total: Int

    // Target: bookmark a computed property (not a function).
    var totalPages: Int {
        return (total + perPage - 1) / perPage
    }

    var hasNextPage: Bool {
        return page < totalPages
    }

    // Target: bookmark a method that takes a closure parameter.
    func mapItems<T, U>(_ items: [T], transform: (T) -> U) -> [U] {
        return items.map(transform)
    }
}

// MARK: - Guard-heavy function (tests bookmarking inside guard blocks)

func parseUserInput(json: [String: Any]) -> User? {
    guard let idString = json["id"] as? String,
          let id = UUID(uuidString: idString) else {
        return nil
    }

    guard let name = json["name"] as? String,
          !name.isEmpty else {
        return nil
    }

    guard let email = json["email"] as? String,
          email.contains("@") else {
        return nil
    }

    // Target: bookmark the return expression after multiple guards.
    let profile = User.Profile(
        bio: json["bio"] as? String ?? "",
        avatarURL: (json["avatar"] as? String).flatMap(URL.init),
        settings: User.Profile.Settings(
            theme: .system,
            notifications: true,
            language: "en"
        )
    )

    return User(id: id, name: name, email: email, profile: profile)
}

// MARK: - Overloaded functions (tests disambiguation)

// Target: bookmark one overload and verify the query disambiguates from the others.
func format(_ value: Int) -> String {
    return "\(value)"
}

func format(_ value: Double) -> String {
    return String(format: "%.2f", value)
}

func format(_ value: String) -> String {
    return value.trimmingCharacters(in: .whitespaces)
}

func format(_ value: Date) -> String {
    let formatter = ISO8601DateFormatter()
    return formatter.string(from: value)
}
