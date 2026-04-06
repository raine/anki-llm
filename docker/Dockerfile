FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV QT_QPA_PLATFORM=offscreen
ENV XDG_RUNTIME_DIR=/tmp/runtime
ENV HOME=/root

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    unzip \
    python3 \
    python3-pip \
    # System PyQt6 (pre-built for aarch64, avoids compilation)
    python3-pyqt6 \
    python3-pyqt6.qtwebengine \
    python3-pyqt6.qtwebchannel \
    python3-pyqt6.qtmultimedia \
    python3-pyqt6.qtquick \
    python3-pyqt6.qtqml \
    python3-pyqt6.qtsvg \
    # Qt/GL runtime deps
    libegl1 \
    libgl1 \
    libnss3 \
    libxcb-cursor0 \
    libxcb-icccm4 \
    libxcb-keysyms1 \
    libxcb-shape0 \
    libxkbcommon0 \
    libdbus-1-3 \
    libxcb-xinerama0 \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p $XDG_RUNTIME_DIR

# Install anki + aqt (skip deps, we provide PyQt6 from system)
RUN pip install --break-system-packages --no-cache-dir --no-deps anki aqt

# Install remaining Python deps
RUN pip install --break-system-packages --no-cache-dir \
    beautifulsoup4 requests flask flask-cors waitress \
    send2trash markdown jsonschema protobuf \
    decorator distro orjson

# Install AnkiConnect add-on
RUN mkdir -p /root/.local/share/Anki2/addons21/2055492159 && \
    curl -sL https://github.com/FooSoft/anki-connect/archive/refs/heads/master.zip -o /tmp/ac.zip && \
    unzip -q /tmp/ac.zip -d /tmp && \
    cp -R /tmp/anki-connect-master/plugin/* /root/.local/share/Anki2/addons21/2055492159/ && \
    rm -rf /tmp/ac.zip /tmp/anki-connect-master

# Configure AnkiConnect to bind 0.0.0.0
RUN echo '{"config": {"webBindAddress": "0.0.0.0", "webBindPort": 8765, "webCorsOriginList": ["*"]}}' \
    > /root/.local/share/Anki2/addons21/2055492159/meta.json

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 8765
ENTRYPOINT ["/entrypoint.sh"]
