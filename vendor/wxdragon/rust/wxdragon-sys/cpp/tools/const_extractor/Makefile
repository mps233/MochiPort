SHELL=/bin/bash # Explicitly set shell for $(shell ...) compatibility

# ==============================================================================
# CMAKE-BASED BUILD WITH PREBUILT WXWIDGETS
# ==============================================================================
# Since wxDragon now uses prebuilt libraries, we use CMake with the provided
# wxWidgetsConfig.cmake for proper configuration instead of manual setup.

BUILD_DIR = build
TARGET_EXEC = const_extractor

.PHONY: all clean configure build

# Default target
all: build

# Configure the build with CMake
configure:
	@echo "Configuring const_extractor with CMake..."
	@mkdir -p $(BUILD_DIR)
	cd $(BUILD_DIR) && cmake ..

# Build the project
build: configure
	@echo "Building const_extractor..."
	cd $(BUILD_DIR) && $(MAKE)
	@if [ -f $(BUILD_DIR)/$(TARGET_EXEC) ]; then \
		cp $(BUILD_DIR)/$(TARGET_EXEC) ./$(TARGET_EXEC); \
		echo "const_extractor built successfully!"; \
	else \
		echo "Build failed - executable not found"; \
		exit 1; \
	fi

# Clean build artifacts
clean:
	@echo "Cleaning build files..."
	rm -rf $(BUILD_DIR)
	rm -f $(TARGET_EXEC)

# For backward compatibility - direct build without intermediate steps
$(TARGET_EXEC): build

# Show help
help:
	@echo "Available targets:"
	@echo "  all        - Configure and build (default)"
	@echo "  configure  - Configure build with CMake"
	@echo "  build      - Build the project"
	@echo "  clean      - Remove build files"
	@echo "  help       - Show this help" 