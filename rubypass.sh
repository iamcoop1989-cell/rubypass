#!/usr/bin/env bash
# RuBypass — обход VPN для российских IP на macOS
set -euo pipefail

RUBYPASS_DIR="$HOME/.rubypass"
SUBNET_FILE="$RUBYPASS_DIR/ru_subnets.txt"
RIPE_URL="https://ftp.ripe.net/pub/stats/ripencc/delegated-ripencc-extended-latest"
ZSHRC="$HOME/.zshrc"
MARKER_START="# RuBypass start"
MARKER_END="# RuBypass end"

# ─── Цвет ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
log()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn() { echo -e "${YELLOW}[!]${NC} $*"; }
err()  { echo -e "${RED}[✗]${NC} $*" >&2; }

# ─── Получить физический шлюз ─────────────────────────────────────────────────
get_gateway() {
    local gw
    gw=$(ipconfig getoption en0 router 2>/dev/null || true)
    if [[ -z "$gw" || "$gw" == "0.0.0.0" ]]; then
        gw=$(route -n get default 2>/dev/null | awk '/gateway:/{print $2}' | head -1 || true)
    fi
    if [[ -z "$gw" || "$gw" == "0.0.0.0" ]]; then
        err "Не удалось определить физический шлюз. Проверьте подключение к роутеру."
        return 1
    fi
    echo "$gw"
}

# ─── INSTALL ──────────────────────────────────────────────────────────────────
cmd_install() {
    mkdir -p "$RUBYPASS_DIR"
    local script_src
    script_src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
    cp "$script_src" "$RUBYPASS_DIR/rubypass.sh"
    chmod +x "$RUBYPASS_DIR/rubypass.sh"
    log "Скрипт установлен в $RUBYPASS_DIR/rubypass.sh"

    # Добавляем алиас в ~/.zshrc, если его ещё нет
    if grep -qF "$MARKER_START" "$ZSHRC" 2>/dev/null; then
        warn "Алиас rubypass уже есть в $ZSHRC"
    else
        {
            echo ""
            echo "$MARKER_START"
            echo "alias rubypass='bash $RUBYPASS_DIR/rubypass.sh'"
            echo "$MARKER_END"
        } >> "$ZSHRC"
        log "Алиас rubypass добавлен в $ZSHRC"
    fi

    echo ""
    warn "Выполните 'source ~/.zshrc' или откройте новый терминал, чтобы использовать команду rubypass."
    warn "Затем запустите: rubypass update && rubypass start"
}

# ─── UNINSTALL ────────────────────────────────────────────────────────────────
cmd_uninstall() {
    # Останавливаем маршруты, если запущены
    if [[ -f "$SUBNET_FILE" ]]; then
        warn "Удаляем активные маршруты..."
        cmd_stop 2>/dev/null || true
    fi

    rm -rf "$RUBYPASS_DIR"
    log "Директория $RUBYPASS_DIR удалена"

    # Удаляем блок алиасов из .zshrc
    if grep -qF "$MARKER_START" "$ZSHRC" 2>/dev/null; then
        local tmp
        tmp=$(mktemp)
        awk "/$MARKER_START/,/$MARKER_END/{next} {print}" "$ZSHRC" > "$tmp"
        mv "$tmp" "$ZSHRC"
        log "Блок алиасов удалён из $ZSHRC"
    else
        warn "Блок алиасов в $ZSHRC не найден — пропускаем"
    fi

    log "RuBypass успешно удалён."
}

# ─── UPDATE ───────────────────────────────────────────────────────────────────
cmd_update() {
    mkdir -p "$RUBYPASS_DIR"
    log "Скачиваем реестр RIPE NCC..."

    local tmp_raw
    tmp_raw=$(mktemp)
    if ! curl -fsSL --progress-bar "$RIPE_URL" -o "$tmp_raw"; then
        rm -f "$tmp_raw"
        err "Не удалось скачать данные с $RIPE_URL"
        exit 1
    fi

    log "Фильтруем и конвертируем российские IPv4-подсети..."

    awk -F'|' '
        $2 == "RU" && $3 == "ipv4" && $4 != "" && $5 != "" {
            ip = $4
            count = $5 + 0
            # Конвертируем количество адресов в длину маски
            bits = 32
            n = count
            while (n > 1) { n = n / 2; bits-- }
            print ip "/" bits
        }
    ' "$tmp_raw" > "$SUBNET_FILE"

    rm -f "$tmp_raw"
    local total
    total=$(wc -l < "$SUBNET_FILE" | tr -d ' ')
    log "Сохранено $total подсетей → $SUBNET_FILE"
    log "Дата обновления: $(date '+%Y-%m-%d %H:%M:%S')"
}

# ─── START ────────────────────────────────────────────────────────────────────
cmd_start() {
    if [[ ! -f "$SUBNET_FILE" ]]; then
        warn "Список подсетей не найден. Запускаем update..."
        cmd_update
    fi

    local gw
    gw=$(get_gateway)
    log "Физический шлюз: $gw"

    local total
    total=$(wc -l < "$SUBNET_FILE" | tr -d ' ')
    log "Добавляем $total маршрутов через $gw (параллельно)..."

    # Параллельно добавляем маршруты; ошибки «exists» игнорируем
    cat "$SUBNET_FILE" | xargs -P 50 -I {} \
        sudo route add -net {} "$gw" 2>/dev/null || true

    log "Маршруты успешно добавлены. Российские сайты теперь обходят VPN."
}

# ─── STOP ─────────────────────────────────────────────────────────────────────
cmd_stop() {
    if [[ ! -f "$SUBNET_FILE" ]]; then
        warn "Список подсетей не найден — нечего удалять."
        return 0
    fi

    local gw
    gw=$(get_gateway)

    local total
    total=$(wc -l < "$SUBNET_FILE" | tr -d ' ')
    log "Удаляем $total маршрутов (параллельно)..."

    cat "$SUBNET_FILE" | xargs -P 50 -I {} \
        sudo route delete -net {} "$gw" 2>/dev/null || true

    log "Маршруты удалены. Весь трафик снова идёт через VPN."
}

# ─── RESTART ──────────────────────────────────────────────────────────────────
cmd_restart() {
    log "Перезапуск..."
    cmd_stop
    cmd_start
}

# ─── STATUS ───────────────────────────────────────────────────────────────────
cmd_status() {
    echo ""
    echo -e "${GREEN}═══ RuBypass Status ═══${NC}"

    # Шлюз
    local gw
    gw=$(get_gateway 2>/dev/null || echo "не определён")
    echo -e "  Шлюз (en0):       ${YELLOW}${gw}${NC}"

    # Список подсетей
    if [[ -f "$SUBNET_FILE" ]]; then
        local total
        total=$(wc -l < "$SUBNET_FILE" | tr -d ' ')
        local mtime
        mtime=$(stat -f '%Sm' -t '%Y-%m-%d %H:%M' "$SUBNET_FILE" 2>/dev/null || echo "неизвестно")
        echo -e "  Подсетей в базе:  ${YELLOW}${total}${NC}"
        echo -e "  Дата обновления:  ${YELLOW}${mtime}${NC}"
    else
        echo -e "  Подсетей в базе:  ${RED}список не загружен${NC}"
    fi

    # Активные маршруты через en0 (исключаем utun-интерфейсы)
    local active
    active=$(netstat -rn 2>/dev/null | grep -v utun | grep -c en0 || echo 0)
    echo -e "  Активных маршрутов через en0: ${YELLOW}${active}${NC}"

    # Статус VPN (наличие utun-интерфейса)
    local vpn_ifaces
    vpn_ifaces=$(netstat -rn 2>/dev/null | grep -o 'utun[0-9]*' | sort -u | tr '\n' ' ' || true)
    if [[ -n "$vpn_ifaces" ]]; then
        echo -e "  VPN:              ${GREEN}активен${NC} (${vpn_ifaces})"
    else
        echo -e "  VPN:              ${RED}не обнаружен${NC}"
    fi

    echo ""
}

# ─── ТОЧКА ВХОДА ──────────────────────────────────────────────────────────────
COMMAND="${1:-}"

case "$COMMAND" in
    install)   cmd_install ;;
    uninstall) cmd_uninstall ;;
    update)    cmd_update ;;
    start)     cmd_start ;;
    stop)      cmd_stop ;;
    restart)   cmd_restart ;;
    status)    cmd_status ;;
    *)
        echo "Использование: rubypass {install|uninstall|update|start|stop|restart|status}"
        echo ""
        echo "  install   — установить скрипт и добавить алиас в ~/.zshrc"
        echo "  uninstall — удалить всё"
        echo "  update    — скачать актуальный список российских IP-подсетей"
        echo "  start     — добавить маршруты (российский трафик в обход VPN)"
        echo "  stop      — удалить маршруты"
        echo "  restart   — перезапустить маршруты"
        echo "  status    — показать текущее состояние"
        exit 1
        ;;
esac
