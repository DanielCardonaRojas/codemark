package auth

import (
	"errors"
	"sync"
	"time"
)

// Target: bookmark a struct
type Claims struct {
	Subject string
	Expiry  time.Time
	Roles   []string
}

// Target: bookmark error variables
var (
	ErrInvalidToken            = errors.New("invalid token")
	ErrExpired                 = errors.New("token expired")
	ErrInsufficientPermissions = errors.New("insufficient permissions")
)

// Target: bookmark an interface
type AuthProvider interface {
	ValidateToken(token string) (*Claims, error)
	RefreshToken(token string) (string, error)
}

// Target: bookmark a struct with methods
type AuthService struct {
	secret     string
	issuer     string
	tokenCache map[string]*Claims
	mu         sync.RWMutex
}

// Target: bookmark a constructor function
func NewAuthService(secret, issuer string) *AuthService {
	return &AuthService{
		secret:     secret,
		issuer:     issuer,
		tokenCache: make(map[string]*Claims),
	}
}

// Target: bookmark a method
func (s *AuthService) ValidateToken(token string) (*Claims, error) {
	s.mu.RLock()
	cached, ok := s.tokenCache[token]
	s.mu.RUnlock()

	if ok {
		if cached.Expiry.Before(time.Now()) {
			s.mu.Lock()
			delete(s.tokenCache, token)
			s.mu.Unlock()
			return nil, ErrExpired
		}
		return cached, nil
	}

	claims, err := s.decode(token)
	if err != nil {
		return nil, err
	}

	s.mu.Lock()
	s.tokenCache[token] = claims
	s.mu.Unlock()

	return claims, nil
}

// Target: bookmark a private method
func (s *AuthService) decode(token string) (*Claims, error) {
	if token == "" {
		return nil, ErrInvalidToken
	}
	return &Claims{
		Subject: "user-1",
		Expiry:  time.Now().Add(time.Hour),
		Roles:   []string{"admin"},
	}, nil
}

// Target: bookmark a method with multiple params
func (s *AuthService) CheckPermission(claims *Claims, required, resource string) error {
	for _, role := range claims.Roles {
		if role == required {
			return nil
		}
	}
	return ErrInsufficientPermissions
}

// Target: bookmark a free function
func CreateDefaultAuthService() *AuthService {
	return NewAuthService("default-secret", "codemark")
}
