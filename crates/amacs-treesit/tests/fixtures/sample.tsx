import React, { useState } from "react";

type Props = { title: string };

export function Counter({ title }: Props) {
  const [count, setCount] = useState(0);
  return (
    <div className="counter">
      <h1>{title}</h1>
      <button onClick={() => setCount(count + 1)}>
        Clicked {count} times
      </button>
    </div>
  );
}
