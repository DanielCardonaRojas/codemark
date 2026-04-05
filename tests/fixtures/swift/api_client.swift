import Foundation

// MARK: - Configuration

struct APIConfig {
    let baseURL: URL
    let timeout: TimeInterval
    let maxRetries: Int
    let retryDelay: TimeInterval

    static let `default` = APIConfig(
        baseURL: URL(string: "https://api.example.com")!,
        timeout: 30,
        maxRetries: 3,
        retryDelay: 1.0
    )
}

// MARK: - Request / Response

enum HTTPMethod: String {
    case get = "GET"
    case post = "POST"
    case put = "PUT"
    case delete = "DELETE"
}

struct APIRequest {
    let path: String
    let method: HTTPMethod
    let headers: [String: String]
    let body: Data?

    init(path: String, method: HTTPMethod = .get, headers: [String: String] = [:], body: Data? = nil) {
        self.path = path
        self.method = method
        self.headers = headers
        self.body = body
    }
}

struct APIResponse {
    let statusCode: Int
    let headers: [String: String]
    let data: Data
}

enum APIError: Error {
    case networkError(underlying: Error)
    case httpError(statusCode: Int, body: Data?)
    case timeout
    case invalidURL
    case decodingError(Error)
}

// MARK: - Client

class APIClient {

    private let config: APIConfig
    private let session: URLSession
    private let authService: AuthService?

    init(config: APIConfig = .default, authService: AuthService? = nil) {
        self.config = config
        self.authService = authService

        let sessionConfig = URLSessionConfiguration.default
        sessionConfig.timeoutIntervalForRequest = config.timeout
        self.session = URLSession(configuration: sessionConfig)
    }

    // Target: bookmark the main request dispatcher — all requests flow through here.
    func send(_ request: APIRequest) async throws -> APIResponse {
        guard let url = URL(string: request.path, relativeTo: config.baseURL) else {
            throw APIError.invalidURL
        }

        var urlRequest = URLRequest(url: url)
        urlRequest.httpMethod = request.method.rawValue
        urlRequest.httpBody = request.body

        for (key, value) in request.headers {
            urlRequest.setValue(value, forHTTPHeaderField: key)
        }

        if let auth = authService {
            let token = try await auth.refreshToken("")
            urlRequest.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        return try await sendWithRetry(urlRequest, attempts: config.maxRetries)
    }

    // Target: bookmark retry logic — rate limiter should wrap this.
    private func sendWithRetry(_ request: URLRequest, attempts: Int) async throws -> APIResponse {
        var lastError: Error?

        for attempt in 0..<attempts {
            do {
                let (data, response) = try await session.data(for: request)

                guard let httpResponse = response as? HTTPURLResponse else {
                    throw APIError.networkError(underlying: URLError(.badServerResponse))
                }

                if (200..<300).contains(httpResponse.statusCode) {
                    let headers = httpResponse.allHeaderFields as? [String: String] ?? [:]
                    return APIResponse(statusCode: httpResponse.statusCode, headers: headers, data: data)
                }

                if httpResponse.statusCode == 429 || httpResponse.statusCode >= 500 {
                    lastError = APIError.httpError(statusCode: httpResponse.statusCode, body: data)
                    if attempt < attempts - 1 {
                        let delay = config.retryDelay * pow(2.0, Double(attempt))
                        try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                        continue
                    }
                }

                throw APIError.httpError(statusCode: httpResponse.statusCode, body: data)
            } catch let error as APIError {
                throw error
            } catch {
                lastError = error
                if attempt < attempts - 1 {
                    let delay = config.retryDelay * pow(2.0, Double(attempt))
                    try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                }
            }
        }

        throw lastError ?? APIError.networkError(underlying: URLError(.unknown))
    }

    // Target: bookmark a generic method (tests type parameter handling).
    func sendAndDecode<T: Decodable>(_ request: APIRequest, as type: T.Type) async throws -> T {
        let response = try await send(request)
        do {
            return try JSONDecoder().decode(T.self, from: response.data)
        } catch {
            throw APIError.decodingError(error)
        }
    }
}

// MARK: - Convenience extensions

extension APIClient {
    // Target: bookmark shorthand methods to test extension resolution.
    func get(_ path: String) async throws -> APIResponse {
        return try await send(APIRequest(path: path, method: .get))
    }

    func post(_ path: String, body: Data) async throws -> APIResponse {
        return try await send(APIRequest(path: path, method: .post, body: body))
    }
}
