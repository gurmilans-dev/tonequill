import { createRoot } from 'react-dom/client';
import { App } from './App';
import { createBridge } from './bridge';
import './style.css';

const root = createRoot(document.getElementById('root')!);
createBridge()
  .then((bridge) => root.render(<App bridge={bridge} />))
  .catch((error) => {
    root.render(
      <main className="boot-error">
        <h1>Tonequill could not start</h1>
        <p>{String(error)}</p>
        <p>Close this window and open Tonequill again.</p>
      </main>,
    );
  });
