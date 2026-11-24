use std::sync::atomic::{AtomicBool, Ordering};

static RUSSIAN: AtomicBool = AtomicBool::new(false);

pub fn set_russian(russian: bool) {
    RUSSIAN.store(russian, Ordering::Relaxed);
}

pub fn text(english: &str) -> &str {
    if !RUSSIAN.load(Ordering::Relaxed) {
        return english;
    }
    match english {
        "Settings" => "Настройки",
        "Browser" => "Браузер",
        "Add Parally to a browser…" => "Добавить Parally в браузер…",
        "Add Parally to a browser" => "Добавить Parally в браузер",
        "1. Open the browser's extensions page, such as chrome://extensions or edge://extensions." => {
            "1. Откройте страницу расширений браузера, например chrome://extensions или edge://extensions."
        }
        "2. Turn on Developer mode." => "2. Включите «Режим разработчика».",
        "3. Click Load unpacked and choose the folder Parally opens. Its path is copied." => {
            "3. Нажмите «Загрузить распакованное» и выберите папку, которую откроет Parally. Путь к ней уже скопирован."
        }
        "Downloads then open in Parally. Click the Parally button in the browser toolbar to turn this off." => {
            "После этого загрузки будут открываться в Parally. Чтобы выключить перехват, нажмите кнопку Parally на панели браузера."
        }
        "Open extension folder" => "Открыть папку расширения",
        "The browser extension is missing from this installation." => {
            "В этой установке нет расширения для браузера."
        }
        "Startup" => "Автозапуск",
        "Start Parally at login" => "Запускать при входе в систему",
        "Could not change startup settings." => "Не удалось изменить автозапуск.",
        "Transparency" => "Прозрачность",
        "Theme" => "Тема",
        "Dark" => "Тёмная",
        "Light" => "Светлая",
        "Language" => "Язык",
        "Scale" => "Масштаб",
        "Auto" => "Авто",
        "Done" => "Готово",
        "Open Window" => "Открыть окно",
        "Exit" => "Выход",
        "Minimize" => "Свернуть",
        "Hide to tray" => "Скрыть в трей",
        "Close" => "Закрыть",
        "Folder preset" => "Папка",
        "Choose folder" => "Выбрать папку",
        "Add folder…" => "Добавить папку…",
        "Browser download" => "Загрузка из браузера",
        "Keep in browser" => "Оставить в браузере",
        "Start download" => "Начать загрузку",
        "Download URL" => "Ссылка для загрузки",
        "Save to" => "Сохранить в",
        "Choose a destination file" => "Выберите файл назначения",
        "Choose a folder preset or enter a full path" => "Выберите папку или введите полный путь",
        "Browse…" => "Обзор…",
        "Connections" => "Соединения",
        "Download" => "Скачать",
        "Downloads" => "Загрузки",
        "No downloads yet." => "Загрузок пока нет.",
        "No active downloads" => "Нет загрузок",
        "Open Folder" => "Открыть папку",
        "Open File" => "Открыть файл",
        "Cancel" => "Отмена",
        "Pause" => "Пауза",
        "Resume" => "Продолжить",
        "Restart" => "Перезапуск",
        "Canceled" => "Отменена",
        "Canceling…" => "Отмена…",
        "Pausing…" => "Приостановка…",
        "Connecting…" => "Подключение…",
        "Checking the server…" => "Проверка сервера…",
        "Downloading" => "Загрузка",
        "Resuming" => "Возобновление",
        "Paused" => "Приостановлена",
        "Restart required" => "Нужен перезапуск",
        "Interrupted" => "Прервана",
        "Complete" => "Готово",
        "Partial data deleted." => "Частичные данные удалены.",
        "saved" => "сохранено",
        "received" => "получено",
        "active" => "активных",
        "active connections" => "соединений",
        "1 connection · restart only" => "1 соединение · докачка недоступна",
        "Server version checked on resume" => "Файл проверяется при продолжении",
        "Restart from the beginning" => "Перезапустить с начала",
        "Cancel download?" => "Отменить загрузку?",
        "Restart download?" => "Перезапустить загрузку?",
        "Cancel & delete partial data" => "Отменить и удалить данные",
        "Restart from beginning" => "Скачать заново",
        "Keep download" => "Оставить загрузку",
        "Parally will stop this download and delete its partial data. The history entry will remain." => {
            "Parally остановит загрузку и удалит частичные данные. Запись останется в истории."
        }
        "Parally will download this file from the beginning and replace its partial data." => {
            "Parally скачает файл с начала и заменит частичные данные."
        }
        "Saving downloads before exit…" => "Сохранение загрузок перед выходом…",
        "The tray icon is unavailable. Use Exit to close Parally." => {
            "Трей недоступен. Чтобы закрыть Parally, нажмите «Выход»."
        }
        "Save download as" => "Сохранить загрузку как",
        "Add folder preset" => "Добавить папку",
        "The server returned a web page. Open the link in your browser and copy the direct file download link." => {
            "Сервер вернул веб-страницу. Откройте ссылку в браузере и скопируйте прямую ссылку на файл."
        }
        "The response body is an HTML page. Copy the direct file download link." => {
            "Сервер прислал HTML-страницу. Скопируйте прямую ссылку на файл."
        }
        "The server did not confirm byte ranges and a strong ETag." => {
            "Сервер не подтвердил диапазоны байтов и версию файла (ETag)."
        }
        "The server stopped honoring the saved byte ranges." => {
            "Сервер перестал поддерживать сохранённые диапазоны."
        }
        "The server reports a different file version, size, or final URL." => {
            "Сервер сообщает другую версию файла, размер или итоговый адрес."
        }
        "No verified byte ranges are saved." => "Проверенных диапазонов для докачки нет.",
        "The saved download is missing or belongs to another link." => {
            "Частичной загрузки нет или она относится к другой ссылке."
        }
        "The partial data is missing or its length no longer matches." => {
            "Частичные данные отсутствуют или имеют другую длину."
        }
        "Parally closed before this download finished." => {
            "Parally закрылся до завершения загрузки."
        }
        "Could not start this download." => "Не удалось запустить загрузку.",
        "Parally is closing." => "Parally закрывается.",
        _ => english,
    }
}
