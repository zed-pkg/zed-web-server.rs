// Progressive enhancement only.
//
// Every page works with this file blocked: the install command is selectable
// text, navigation is ordinary links, and the service worker only ever adds an
// offline fallback. Nothing here is required to read or publish a package,
// which is why it can be a single unbundled file under `script-src 'self'`.

(function () {
  "use strict";

  /** Attach a copy button beside every command snippet that asks for one. */
  function wireCopyButtons(root) {
    var snippets = root.querySelectorAll("[data-copy]");
    for (var i = 0; i < snippets.length; i += 1) {
      var snippet = snippets[i];
      if (snippet.dataset.copyWired === "true") {
        continue;
      }
      snippet.dataset.copyWired = "true";
      addCopyButton(snippet);
    }
  }

  function addCopyButton(snippet) {
    // The clipboard API is unavailable on insecure origins and in some
    // embedded webviews. Without it the button would be a lie, so it is never
    // rendered and the command stays selectable.
    if (!navigator.clipboard || !navigator.clipboard.writeText) {
      return;
    }

    var button = document.createElement("button");
    button.type = "button";
    button.className = "copy";
    button.textContent = "Copy";
    button.setAttribute("aria-label", "Copy command to clipboard");

    button.addEventListener("click", function () {
      var text = snippet.getAttribute("data-copy") || snippet.textContent || "";
      navigator.clipboard.writeText(text.trim()).then(
        function () {
          announce(button, "Copied", true);
        },
        function () {
          announce(button, "Press Ctrl+C", false);
        }
      );
    });

    var row = snippet.parentNode;
    if (row && row.classList && row.classList.contains("snippet-row")) {
      row.appendChild(button);
    }
  }

  function announce(button, label, copied) {
    button.textContent = label;
    button.dataset.copied = copied ? "true" : "false";
    window.setTimeout(function () {
      button.textContent = "Copy";
      delete button.dataset.copied;
    }, 1600);
  }

  /** Mark the tab-bar entry matching the current path. */
  function markCurrentTab() {
    var path = window.location.pathname;
    var tabs = document.querySelectorAll(".tabbar a[data-match]");
    for (var i = 0; i < tabs.length; i += 1) {
      var prefix = tabs[i].getAttribute("data-match");
      var active = prefix === "/" ? path === "/" : path.indexOf(prefix) === 0;
      if (active) {
        tabs[i].setAttribute("aria-current", "page");
      } else {
        tabs[i].removeAttribute("aria-current");
      }
    }
  }

  function registerServiceWorker() {
    if (!("serviceWorker" in navigator)) {
      return;
    }
    // Registration failure is never surfaced: the site is fully functional
    // without it, and an error toast about caching would only be noise.
    navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch(function () {});
  }

  function start() {
    wireCopyButtons(document);
    markCurrentTab();
    registerServiceWorker();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }

  // HTMX swaps fragments in without a page load, so newly arrived snippets
  // need wiring too.
  document.body.addEventListener("htmx:afterSwap", function (event) {
    var target = event && event.target ? event.target : document;
    wireCopyButtons(target);
  });
})();
