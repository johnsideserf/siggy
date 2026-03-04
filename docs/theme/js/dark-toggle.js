// Dark mode toggle for siggy docs
// Injects a sun/moon toggle into the menu bar and persists preference.
(function () {
    'use strict';

    var STORAGE_KEY = 'irc-dark-mode';

    function isDark() {
        try {
            return localStorage.getItem(STORAGE_KEY) === 'true';
        } catch (e) {
            return false;
        }
    }

    function applyMode(dark) {
        document.documentElement.setAttribute('data-irc-mode', dark ? 'dark' : 'light');
        try {
            localStorage.setItem(STORAGE_KEY, dark ? 'true' : 'false');
        } catch (e) { /* ignore */ }
    }

    // Apply immediately (before paint) to prevent flash
    applyMode(isDark());

    document.addEventListener('DOMContentLoaded', function () {
        var btn = document.createElement('button');
        btn.className = 'icon-button irc-dark-toggle';
        btn.type = 'button';
        btn.title = 'Toggle dark mode';
        btn.setAttribute('aria-label', 'Toggle dark mode');
        btn.innerHTML = isDark()
            ? '<span class="toggle-icon">&#9788;</span>'   // sun
            : '<span class="toggle-icon">&#9790;</span>';  // moon

        btn.addEventListener('click', function () {
            var dark = !isDark();
            applyMode(dark);
            btn.innerHTML = dark
                ? '<span class="toggle-icon">&#9788;</span>'
                : '<span class="toggle-icon">&#9790;</span>';
        });

        // Insert into the left-buttons area, after the sidebar toggle
        var leftButtons = document.querySelector('.left-buttons');
        if (leftButtons) {
            leftButtons.appendChild(btn);
        }
    });
})();
