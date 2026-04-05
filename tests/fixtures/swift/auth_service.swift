import Foundation

// MARK: - Models

struct Claims: Codable {
    let subject: String
    let expiry: Date
    let roles: [String]
}

enum AuthError: Error {
    case invalidToken
    case expired
    case insufficientPermissions(required: String)
}

// MARK: - Protocol

protocol AuthProvider {
    func validateToken(_ token: String) async throws -> Claims
    func refreshToken(_ token: String) async throws -> String
}

// MARK: - Implementation

class AuthService: AuthProvider {

    private let secret: String
    private let issuer: String
    private var tokenCache: [String: Claims] = [:]

    init(secret: String, issuer: String = "codemark") {
        self.secret = secret
        self.issuer = issuer
    }

    // Target: bookmark this function, rename it, and verify resolution still works.
    func validateToken(_ token: String) async throws -> Claims {
        if let cached = tokenCache[token] {
            guard cached.expiry > Date() else {
                tokenCache.removeValue(forKey: token)
                throw AuthError.expired
            }
            return cached
        }

        let claims = try decode(token)

        guard claims.expiry > Date() else {
            throw AuthError.expired
        }

        tokenCache[token] = claims
        return claims
    }

    // Target: bookmark this, then extract it to a separate file and verify cross-file resolution.
    func refreshToken(_ token: String) async throws -> String {
        let claims = try decode(token)

        guard claims.expiry > Date().addingTimeInterval(-3600) else {
            throw AuthError.expired
        }

        let newClaims = Claims(
            subject: claims.subject,
            expiry: Date().addingTimeInterval(3600),
            roles: claims.roles
        )

        return try encode(newClaims)
    }

    // Target: bookmark a private method nested inside a class.
    private func decode(_ token: String) throws -> Claims {
        guard let data = Data(base64Encoded: token) else {
            throw AuthError.invalidToken
        }
        return try JSONDecoder().decode(Claims.self, from: data)
    }

    private func encode(_ claims: Claims) throws -> String {
        let data = try JSONEncoder().encode(claims)
        return data.base64EncodedString()
    }

    // Target: bookmark a method with complex parameters for disambiguation testing.
    func checkPermission(_ claims: Claims, required: String, resource: String) throws -> Bool {
        guard claims.roles.contains(required) else {
            throw AuthError.insufficientPermissions(required: required)
        }
        return true
    }
}

// MARK: - Extension

extension AuthService {
    // Target: bookmark a method inside an extension.
    func invalidateCache() {
        tokenCache.removeAll()
    }

    func cacheSize() -> Int {
        return tokenCache.count
    }
}

// MARK: - Free function (top-level)

// Target: bookmark a top-level function (no class/struct parent).
func createDefaultAuthService() -> AuthService {
    return AuthService(secret: "default-secret")
}
