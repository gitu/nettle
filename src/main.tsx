import React from 'react';
import ReactDOM from 'react-dom/client';
// schr.ag design-system fonts: JetBrains Mono (mono/console voice) + Instrument
// Sans (UI). IBM Plex kept installed as a fallback in the font stack.
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import '@fontsource/jetbrains-mono/600.css';
import '@fontsource/instrument-sans/400.css';
import '@fontsource/instrument-sans/500.css';
import '@fontsource/instrument-sans/600.css';
import './styles.css';
import App from './App';
import { initStore } from './store';

initStore().catch(console.error);

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
