(function() {
  const SB_KEY = 'codetours.sidebar.collapsed';
  const root = document.documentElement;
  
  // Initialize sidebar state
  if (localStorage.getItem(SB_KEY) === '1') {
    root.dataset.sidebarCollapsed = '1';
  }

  document.addEventListener('DOMContentLoaded', () => {
    // Sidebar toggle
    const toggle = document.querySelector('[data-sidebar-toggle]');
    const sidebar = document.querySelector('.sidebar');
    if (toggle && sidebar) {
      toggle.addEventListener('click', () => {
        const collapsed = sidebar.dataset.sidebarCollapsed === 'true';
        sidebar.dataset.sidebarCollapsed = collapsed ? 'false' : 'true';
        localStorage.setItem(SB_KEY, collapsed ? '0' : '1');
      });
    }

    // Scroll Spy
    const scrollContainer = document.querySelector('[data-scrollspy]');
    if (scrollContainer) {
      const items = document.querySelectorAll('.step-item');
      const panels = document.querySelectorAll('.metadata-sidebar [data-step-panel]');
      
      const observer = new IntersectionObserver((entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            const ord = entry.target.dataset.stepOrdinal;
            root.dataset.activeStep = ord;
            
            items.forEach(item => {
              if (item.dataset.stepNav === ord) {
                item.classList.add('active');
              } else {
                item.classList.remove('active');
              }
            });
            
            panels.forEach(panel => {
              if (panel.dataset.stepPanel === ord) {
                panel.classList.remove('hidden');
              } else {
                panel.classList.add('hidden');
              }
            });
            
            break; // Process the first intersecting item in the batch
          }
        }
      }, { root: scrollContainer, rootMargin: '-150px 0px -50% 0px', threshold: 0 });
      
      document.querySelectorAll('.step-block').forEach(block => {
        observer.observe(block);
      });
      
      // Left sidebar click handler
      const nav = document.querySelector('.steps-sidebar nav') || document.querySelector('aside.w-64 nav');
      if (nav) {
        nav.addEventListener('click', (e) => {
          const btn = e.target.closest('.step-item');
          if (btn) {
            const ord = btn.dataset.stepNav;
            const target = document.getElementById('step-' + ord);
            if (target) {
              target.scrollIntoView({ behavior: 'smooth', block: 'start' });
            }
          }
        });
      }

      // Hash fragment handling on load
      if (window.location.hash && window.location.hash.startsWith('#step-')) {
        const targetId = window.location.hash.substring(1);
        const target = document.getElementById(targetId);
        if (target) {
          setTimeout(() => target.scrollIntoView({ behavior: 'smooth', block: 'start' }), 100);
        }
      }
    }

    // Editor URL builders
    function buildEditorUrl(editor, data) {
      const line = data.line || 1;
      const path = data.path || "";
      switch (editor) {
        case 'cursor': return `cursor://file/${path}:${line}:1`;
        case 'idea': return `idea://open?file=${path}&line=${line}&column=1`;
        case 'vscode': default: return `vscode://file/${path}:${line}:1`;
      }
    }

    document.body.addEventListener('click', (e) => {
      // Open in Editor
      const editorBtn = e.target.closest('[data-open-in-editor]');
      if (editorBtn) {
        let editor = localStorage.getItem('codetours.editor');
        if (!editor) {
          editor = window.prompt("Which editor do you use? (vscode, cursor, idea)", "vscode") || "vscode";
          localStorage.setItem('codetours.editor', editor);
        }
        window.location.href = buildEditorUrl(editor, editorBtn.dataset);
        return;
      }

      // Copy to clipboard
      const copyBtn = e.target.closest('[data-copy]');
      if (copyBtn) {
        let textToCopy = copyBtn.dataset.copy;
        // Make relative permalink absolute
        if (textToCopy.startsWith('/tours/')) {
          textToCopy = window.location.origin + textToCopy;
        }
        navigator.clipboard.writeText(textToCopy).then(() => {
          const originalContent = copyBtn.innerHTML;
          copyBtn.innerHTML = '<span class="text-status-healthy text-[10px] font-bold px-1">Copied!</span>';
          setTimeout(() => {
            copyBtn.innerHTML = originalContent;
          }, 2000);
        });
        return;
      }
    });
  });
})();