package com.example.auth

// Target: bookmark a data class
data class Claims(
    val subject: String,
    val expiry: Long,
    val roles: List<String>
)

// Target: bookmark a sealed class
sealed class AuthError(val message: String) {
    object InvalidToken : AuthError("invalid token")
    object Expired : AuthError("token expired")
    data class InsufficientPermissions(val required: String) :
        AuthError("insufficient permissions: $required")
}

// Target: bookmark an interface
interface AuthProvider {
    fun validateToken(token: String): Claims
    fun refreshToken(token: String): String
}

// Target: bookmark a class
class AuthService(
    private val secret: String,
    private val issuer: String = "codemark"
) : AuthProvider {

    private val tokenCache = mutableMapOf<String, Claims>()

    // Target: bookmark a method
    override fun validateToken(token: String): Claims {
        tokenCache[token]?.let { cached ->
            if (cached.expiry <= System.currentTimeMillis()) {
                tokenCache.remove(token)
                throw IllegalStateException(AuthError.Expired.message)
            }
            return cached
        }

        val claims = decode(token)
        tokenCache[token] = claims
        return claims
    }

    override fun refreshToken(token: String): String {
        val claims = decode(token)
        return encode(claims)
    }

    // Target: bookmark a private method
    private fun decode(token: String): Claims {
        if (token.isEmpty()) {
            throw IllegalArgumentException(AuthError.InvalidToken.message)
        }
        return Claims(
            subject = "user-1",
            expiry = System.currentTimeMillis() + 3600000,
            roles = listOf("admin")
        )
    }

    private fun encode(claims: Claims): String {
        return "$issuer:${claims.subject}:$secret"
    }

    // Target: bookmark a method with complex params
    fun checkPermission(claims: Claims, required: String, resource: String) {
        if (required !in claims.roles) {
            throw SecurityException(
                AuthError.InsufficientPermissions(required).message
            )
        }
    }

    companion object {
        // Target: bookmark a companion object factory
        fun createDefault(): AuthService {
            return AuthService("default-secret")
        }
    }
}

// Target: bookmark a top-level function
fun createDefaultAuthService(): AuthService {
    return AuthService.createDefault()
}
