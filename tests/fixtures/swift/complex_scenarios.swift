import Foundation

@propertyWrapper
struct Trimmed {
    private(set) var value: String = ""
    var wrappedValue: String {
        get { value }
        set { value = newValue.trimmingCharacters(in: .whitespacesAndNewlines) }
    }
}

protocol ViewModelProtocol {
    associatedtype State
    func update(state: State)
}

class ComplexViewModel<S: Equatable>: ViewModelProtocol {
    typealias State = S
    
    @Trimmed var identifier: String
    private let queue = DispatchQueue(label: "com.codemark.test")
    
    init(identifier: String) {
        self.identifier = identifier
    }
    
    func update(state: State) {
        queue.async { [weak self] in
            guard let self = self else { return }
            print("Updating state for \(self.identifier)")
            
            // Nested closure and complex expression
            let result = [1, 2, 3].map { $0 * 2 }.filter { $0 > 2 }
            if result.contains(4) {
                self.performSideEffect()
            }
        }
    }
    
    private func performSideEffect() {
        // Target for fine-grained
    }
}

extension ComplexViewModel where S == String {
    func specializedAction() {
        let message = "Specialized for String"
        print(message)
    }
}

enum NetworkError: Error {
    case unauthorized
    case serverError(code: Int)
    
    var isRetryable: Bool {
        switch self {
        case .serverError(let code) where code >= 500:
            return true
        default:
            return false
        }
    }
}
