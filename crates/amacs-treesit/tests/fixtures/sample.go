package main

import (
    "fmt"
    "os"
)

type Shape interface {
    Area() float64
}

type Rect struct {
    W, H float64
}

func (r Rect) Area() float64 {
    return r.W * r.H
}

func main() {
    shapes := []Shape{
        Rect{1, 2},
        Rect{3, 4},
    }
    for i, s := range shapes {
        switch {
        case s.Area() > 5:
            fmt.Println(i, "big")
        default:
            fmt.Println(i, "small")
        }
    }
    if len(os.Args) > 1 {
        fmt.Printf("%s\n", os.Args[1])
    }
}
